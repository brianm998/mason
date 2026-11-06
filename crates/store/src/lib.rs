// SPDX-License-Identifier: GPL-3.0-or-later

//! Durable append-only event log with projections, on SQLite in WAL mode.
//!
//! One writer thread owns the write connection. An append reduces the event,
//! writes the log row and the projection rows, and commits, all in one
//! transaction, so a crash leaves either all of it or none. Readers use a
//! separate read connection. Projections can always be rebuilt from the log.
//!
//! The API is blocking; async callers use a bounded worker pool (Part 2).

mod codec;
pub mod migrations;

use mason_core::*;
use mason_engine::{reduce, ReduceError, World};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Sender};
use std::sync::Mutex;
use std::thread::JoinHandle;

pub use codec::{decode as decode_event, encode as encode_event, state_name, CodecError};

#[derive(Debug)]
pub enum StoreError {
    Sqlite(rusqlite::Error),
    /// The reducer refused the event. Nothing was written.
    Rejected(ReduceError),
    Codec(CodecError),
    /// The log does not replay cleanly. Carries the sequence number.
    Corrupt {
        seq: u64,
        why: String,
    },
    /// Projections disagree with the log.
    Inconsistent(String),
    WriterGone,
}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        StoreError::Sqlite(e)
    }
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for StoreError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredEvent {
    pub seq: u64,
    pub event: Event,
}

/// One thing the owner did to the pipeline (an override, an answer, an
/// approval): what the "interventions per landed task" metric counts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InterventionRow {
    pub id: u64,
    pub at: u64,
    pub kind: String,
    pub task: Option<u64>,
    pub project: String,
    pub who: String,
    pub detail: String,
    pub reason: String,
}

/// A pause in force: `project` is empty for the whole pipeline; `mode` is
/// `all`, `claims` or `landing`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PauseRow {
    pub id: u64,
    pub project: String,
    pub mode: String,
    pub owner: String,
    pub created_at: u64,
    pub expires_at: u64,
}

/// An alert tied to a live condition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AlertRow {
    pub key: String,
    pub kind: String,
    pub text: String,
    pub since: u64,
    pub cleared_at: Option<u64>,
}

/// A debt: something skipped by force and not yet paid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DebtRow {
    pub id: u64,
    pub at: u64,
    pub task: u64,
    pub project: String,
    /// `suite:NAME` or `step:NAME`.
    pub scope: String,
    pub reason: String,
    pub owner: String,
    /// Run it again after landing.
    pub deferred: bool,
    /// `open` or `paid`.
    pub status: String,
}

/// A batch the landing engine formed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BatchRow {
    pub id: u64,
    pub project: String,
    pub at: u64,
    pub finished_at: Option<u64>,
    /// `forming`, `merged`, `verifying`, `red`, `promoting`, `landed`, `parked`, `failed`.
    pub phase: String,
    /// Task ids, comma separated, in landing order.
    pub tasks: String,
    pub landed: String,
    pub parked: String,
    pub note: String,
    pub head: String,
}

/// Something a batch put on the outside: a candidate ref or a pull request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefRow {
    pub id: u64,
    pub batch: u64,
    pub project: String,
    /// `ci` or `pr`.
    pub kind: String,
    /// The branch name on the remote.
    pub name: String,
    pub number: Option<u64>,
}

/// A tracker item imported as a task, with what the tracker last said.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackerItemRow {
    pub project: String,
    pub number: u64,
    pub task: u64,
    pub title: String,
    pub body: String,
    pub priority: String,
    pub open: bool,
    /// The task state last mirrored out to the tracker (`landed`, `failed`, ...).
    pub mirrored: String,
}

/// A project's landing freeze: its post-land tier is red, so only the fix may land.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FreezeRow {
    pub project: String,
    pub since: u64,
    pub reason: String,
    pub fix_task: Option<u64>,
}

/// A runtime configuration override. Every one has an owner and an expiry:
/// switches expire (Part 1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OverrideRow {
    pub id: i64,
    pub project: String,
    pub key: String,
    pub value: String,
    pub owner: String,
    pub created_at: u64,
    pub expires_at: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Appended {
    pub seq: u64,
    pub effects: Vec<Effect>,
}

enum Cmd {
    Append(Event, Sender<Result<Appended, StoreError>>),
    World(Sender<World>),
    Rebuild(Sender<Result<World, StoreError>>),
    Bug(u64, String, String, Sender<Result<(), StoreError>>),
    /// Run a write on the writer thread, so there is still only one writer.
    Write(Box<dyn FnOnce(&Connection) + Send>),
    Shutdown,
}

pub struct Store {
    tx: Sender<Cmd>,
    reader: Mutex<Connection>,
    writer: Option<JoinHandle<()>>,
    path: PathBuf,
}

fn open_write(path: &Path) -> Result<Connection, StoreError> {
    let mut conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "FULL")?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    migrations::migrate_to(&mut conn, migrations::SCHEMA_VERSION)?;
    Ok(conn)
}

fn read_events(
    conn: &Connection,
    after: u64,
    limit: usize,
) -> Result<Vec<StoredEvent>, StoreError> {
    let mut st = conn.prepare(
        "SELECT seq, at, kind, payload FROM events WHERE seq > ?1 ORDER BY seq LIMIT ?2",
    )?;
    let rows = st.query_map(params![after as i64, limit as i64], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
        ))
    })?;
    let mut out = vec![];
    for row in rows {
        let (seq, at, kind, payload) = row?;
        let kind = codec::decode(&kind, &payload).map_err(StoreError::Codec)?;
        out.push(StoredEvent {
            seq: seq as u64,
            event: Event {
                at: Timestamp(at as u64),
                kind,
            },
        });
    }
    Ok(out)
}

fn last_seq(conn: &Connection) -> Result<u64, StoreError> {
    let n: Option<i64> = conn.query_row("SELECT MAX(seq) FROM events", [], |r| r.get(0))?;
    Ok(n.unwrap_or(0) as u64)
}

/// Replay the whole log through the reducer.
fn replay(conn: &Connection) -> Result<World, StoreError> {
    let mut world = World::new();
    let mut after = 0;
    loop {
        let batch = read_events(conn, after, 5000)?;
        let Some(last) = batch.last() else {
            return Ok(world);
        };
        after = last.seq;
        for se in batch {
            world = reduce(&world, &se.event)
                .map_err(|e| StoreError::Corrupt {
                    seq: se.seq,
                    why: format!("{e:?}"),
                })?
                .0;
        }
    }
}

fn activity_parts(a: &Activity) -> (&'static str, String) {
    match a {
        Activity::Idle => ("idle", String::new()),
        Activity::Running => ("running", String::new()),
        Activity::Waiting(w) => ("waiting", format!("{w:?}")),
        Activity::Held(b) => ("held", format!("{b:?}")),
        Activity::Paused => ("paused", String::new()),
    }
}

fn task_line(t: &Task) -> String {
    let (k, d) = activity_parts(&t.activity);
    format!(
        "task {}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
        t.id.0,
        t.project.0,
        t.title,
        codec::state_name(t.state),
        k,
        d,
        codec::priority_name(t.priority),
        t.enqueued_at.0,
        t.failures,
        t.decision.map_or("-".into(), |d| d.0.to_string()),
        t.step,
        t.route.map_or("", |r| r.name()),
        t.def,
    )
}

/// What the projection tables should contain for `w`, as sorted lines.
fn dump_world(w: &World) -> Vec<String> {
    let mut v: Vec<String> = vec![];
    v.extend(
        w.projects
            .values()
            .map(|p| format!("project {}|{}", p.id.0, p.name)),
    );
    v.extend(w.tasks.values().map(task_line));
    v.extend(w.decisions.values().map(|d| {
        format!(
            "decision {}|{}|{}|{}",
            d.id.0,
            d.task.0,
            d.question,
            d.answer.as_deref().unwrap_or("-")
        )
    }));
    v.sort();
    v
}

/// What the projection tables actually contain, in the same format.
fn dump_db(conn: &Connection) -> Result<Vec<String>, StoreError> {
    let mut v: Vec<String> = vec![];
    let mut st = conn.prepare("SELECT id, name FROM projects")?;
    for r in st.query_map([], |r| {
        Ok(format!(
            "project {}|{}",
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?
        ))
    })? {
        v.push(r?);
    }
    let mut st = conn.prepare(
        "SELECT id, project, title, state, activity_kind, activity_detail, priority,
                enqueued_at, failures, decision, step, route, def FROM tasks",
    )?;
    let rows = st.query_map([], |r| {
        let dec: Option<i64> = r.get(9)?;
        Ok(format!(
            "task {}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
            r.get::<_, i64>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, String>(5)?,
            r.get::<_, String>(6)?,
            r.get::<_, i64>(7)?,
            r.get::<_, i64>(8)?,
            dec.map_or("-".into(), |d| d.to_string()),
            r.get::<_, String>(10)?,
            r.get::<_, String>(11)?,
            r.get::<_, String>(12)?,
        ))
    })?;
    for r in rows {
        v.push(r?);
    }
    let mut st = conn.prepare("SELECT id, task, question, answer FROM decisions")?;
    let rows = st.query_map([], |r| {
        let a: Option<String> = r.get(3)?;
        Ok(format!(
            "decision {}|{}|{}|{}",
            r.get::<_, i64>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, String>(2)?,
            a.unwrap_or_else(|| "-".into())
        ))
    })?;
    for r in rows {
        v.push(r?);
    }
    v.sort();
    Ok(v)
}

fn write_project(conn: &Connection, p: &Project) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO projects(id, name) VALUES (?1, ?2)",
        params![p.id.0 as i64, p.name],
    )?;
    Ok(())
}

fn write_task(conn: &Connection, t: &Task) -> rusqlite::Result<()> {
    let (k, d) = activity_parts(&t.activity);
    conn.execute(
        "INSERT OR REPLACE INTO tasks(id, project, title, state, activity_kind, activity_detail,
                                      priority, enqueued_at, failures, decision, step, route, def)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            t.id.0 as i64,
            t.project.0 as i64,
            t.title,
            codec::state_name(t.state),
            k,
            d,
            codec::priority_name(t.priority),
            t.enqueued_at.0 as i64,
            t.failures,
            t.decision.map(|d| d.0 as i64),
            t.step,
            t.route.map_or("", |r| r.name()),
            t.def,
        ],
    )?;
    Ok(())
}

fn write_decision(conn: &Connection, d: &Decision) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO decisions(id, task, question, answer) VALUES (?1, ?2, ?3, ?4)",
        params![d.id.0 as i64, d.task.0 as i64, d.question, d.answer],
    )?;
    Ok(())
}

/// Write only the rows that differ between `old` and `new`.
fn project_diff(conn: &Connection, old: &World, new: &World) -> rusqlite::Result<()> {
    for p in new
        .projects
        .values()
        .filter(|p| old.projects.get(&p.id) != Some(p))
    {
        write_project(conn, p)?;
    }
    for t in new
        .tasks
        .values()
        .filter(|t| old.tasks.get(&t.id) != Some(t))
    {
        write_task(conn, t)?;
    }
    for d in new
        .decisions
        .values()
        .filter(|d| old.decisions.get(&d.id) != Some(d))
    {
        write_decision(conn, d)?;
    }
    Ok(())
}

fn set_projected(conn: &Connection, seq: u64) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO meta(key, value) VALUES ('projected_seq', ?1)",
        params![seq.to_string()],
    )?;
    Ok(())
}

fn projected_seq(conn: &Connection) -> Result<u64, StoreError> {
    let v: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'projected_seq'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    Ok(v.and_then(|s| s.parse().ok()).unwrap_or(0))
}

/// Drop and rewrite every projection from the log, in one transaction.
fn rebuild(conn: &mut Connection) -> Result<World, StoreError> {
    let tx = conn.transaction()?;
    let world = replay(&tx)?;
    tx.execute_batch("DELETE FROM decisions; DELETE FROM tasks; DELETE FROM projects;")?;
    project_diff(&tx, &World::new(), &world)?;
    set_projected(&tx, last_seq(&tx)?)?;
    tx.commit()?;
    Ok(world)
}

fn writer_loop(mut conn: Connection, mut world: World, rx: std::sync::mpsc::Receiver<Cmd>) {
    while let Ok(cmd) = rx.recv() {
        match cmd {
            Cmd::Shutdown => break,
            Cmd::World(reply) => {
                let _ = reply.send(world.clone());
            }
            Cmd::Rebuild(reply) => {
                let r = rebuild(&mut conn);
                if let Ok(w) = &r {
                    world = w.clone();
                }
                let _ = reply.send(r);
            }
            Cmd::Write(f) => f(&conn),
            Cmd::Bug(at, kind, detail, reply) => {
                let r = conn
                    .execute(
                        "INSERT INTO bugs(at, kind, detail) VALUES (?1, ?2, ?3)",
                        params![at as i64, kind, detail],
                    )
                    .map(|_| ())
                    .map_err(StoreError::from);
                let _ = reply.send(r);
            }
            Cmd::Append(ev, reply) => match append_one(&mut conn, &world, &ev) {
                Ok((next, appended)) => {
                    world = next;
                    let _ = reply.send(Ok(appended));
                }
                Err(e) => {
                    let _ = reply.send(Err(e));
                }
            },
        }
    }
}

fn append_one(
    conn: &mut Connection,
    world: &World,
    ev: &Event,
) -> Result<(World, Appended), StoreError> {
    let (next, effects) = reduce(world, ev).map_err(StoreError::Rejected)?;
    let (kind, payload) = codec::encode(&ev.kind);
    let tx = conn.transaction()?;
    tx.execute(
        "INSERT INTO events(at, kind, payload) VALUES (?1, ?2, ?3)",
        params![ev.at.0 as i64, kind, payload],
    )?;
    let seq = tx.last_insert_rowid() as u64;
    project_diff(&tx, world, &next)?;
    set_projected(&tx, seq)?;
    tx.commit()?;
    Ok((next, Appended { seq, effects }))
}

impl Store {
    /// Open or create the database, migrate it, and bring projections up to
    /// date with the log.
    pub fn open(path: impl AsRef<Path>) -> Result<Store, StoreError> {
        let path = path.as_ref().to_path_buf();
        let mut conn = open_write(&path)?;
        let world = if projected_seq(&conn)? != last_seq(&conn)? {
            rebuild(&mut conn)?
        } else {
            replay(&conn)?
        };
        let reader = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        reader.pragma_update(None, "busy_timeout", 5000)?;
        let (tx, rx) = channel();
        let writer = std::thread::Builder::new()
            .name("mason-store-writer".into())
            .spawn(move || writer_loop(conn, world, rx))
            .expect("spawn writer thread");
        Ok(Store {
            tx,
            reader: Mutex::new(reader),
            writer: Some(writer),
            path,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append an event. Returns once it is durably committed together with
    /// its projection rows. A refused event writes nothing.
    pub fn append(&self, event: Event) -> Result<Appended, StoreError> {
        let (rtx, rrx) = channel();
        self.tx
            .send(Cmd::Append(event, rtx))
            .map_err(|_| StoreError::WriterGone)?;
        rrx.recv().map_err(|_| StoreError::WriterGone)?
    }

    /// Events with `seq > after`, oldest first. A client resumes from the
    /// last sequence number it saw.
    pub fn read_since(&self, after: u64, limit: usize) -> Result<Vec<StoredEvent>, StoreError> {
        let conn = self.reader.lock().expect("reader lock");
        read_events(&conn, after, limit)
    }

    pub fn last_seq(&self) -> Result<u64, StoreError> {
        let conn = self.reader.lock().expect("reader lock");
        last_seq(&conn)
    }

    /// The current in-memory world, as the writer sees it.
    pub fn world(&self) -> Result<World, StoreError> {
        let (rtx, rrx) = channel();
        self.tx
            .send(Cmd::World(rtx))
            .map_err(|_| StoreError::WriterGone)?;
        rrx.recv().map_err(|_| StoreError::WriterGone)
    }

    /// Rebuild every projection from the log.
    pub fn rebuild_projections(&self) -> Result<World, StoreError> {
        let (rtx, rrx) = channel();
        self.tx
            .send(Cmd::Rebuild(rtx))
            .map_err(|_| StoreError::WriterGone)?;
        rrx.recv().map_err(|_| StoreError::WriterGone)?
    }

    /// Run `f` as a write on the single writer thread and return its result.
    /// For writes that are not events: definitions, overrides, and the like.
    pub fn write<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Connection) -> rusqlite::Result<T> + Send + 'static,
    ) -> Result<T, StoreError> {
        let (rtx, rrx) = channel();
        self.tx
            .send(Cmd::Write(Box::new(move |c| {
                let _ = rtx.send(f(c));
            })))
            .map_err(|_| StoreError::WriterGone)?;
        rrx.recv()
            .map_err(|_| StoreError::WriterGone)?
            .map_err(StoreError::from)
    }

    /// Run a read on the read connection.
    pub fn read<T>(
        &self,
        f: impl FnOnce(&Connection) -> rusqlite::Result<T>,
    ) -> Result<T, StoreError> {
        let conn = self.reader.lock().expect("reader lock");
        f(&conn).map_err(StoreError::from)
    }

    /// Remember a definition by its hash, so a task pinned to it can always find it.
    pub fn put_definition(&self, hash: &str, text: &str, at_ms: u64) -> Result<(), StoreError> {
        let (h, t) = (hash.to_string(), text.to_string());
        self.write(move |c| {
            c.execute(
                "INSERT OR IGNORE INTO definitions(hash, text, at) VALUES (?1, ?2, ?3)",
                params![h, t, at_ms as i64],
            )
            .map(|_| ())
        })
    }

    pub fn definition(&self, hash: &str) -> Result<Option<String>, StoreError> {
        self.read(|c| {
            c.query_row(
                "SELECT text FROM definitions WHERE hash = ?1",
                params![hash],
                |r| r.get(0),
            )
            .optional()
        })
    }

    /// Set a runtime override for a project (replacing one for the same key).
    /// It must have an owner and expire in the future.
    pub fn add_override(
        &self,
        project: &str,
        key: &str,
        value: &str,
        owner: &str,
        now_ms: u64,
        expires_at_ms: u64,
    ) -> Result<i64, StoreError> {
        if owner.trim().is_empty() {
            return Err(StoreError::Inconsistent(
                "an override needs an owner".into(),
            ));
        }
        if expires_at_ms <= now_ms {
            return Err(StoreError::Inconsistent(
                "an override must expire in the future".into(),
            ));
        }
        let (p, k, v, o) = (
            project.to_string(),
            key.to_string(),
            value.to_string(),
            owner.to_string(),
        );
        self.write(move |c| {
            c.execute("DELETE FROM overrides WHERE project = ?1 AND key = ?2", params![p, k])?;
            c.execute(
                "INSERT INTO overrides(project, key, value, owner, created_at, expires_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![p, k, v, o, now_ms as i64, expires_at_ms as i64],
            )?;
            Ok(c.last_insert_rowid())
        })
    }

    pub fn remove_override(&self, project: &str, key: &str) -> Result<usize, StoreError> {
        let (p, k) = (project.to_string(), key.to_string());
        self.write(move |c| {
            c.execute(
                "DELETE FROM overrides WHERE project = ?1 AND key = ?2",
                params![p, k],
            )
        })
    }

    /// Overrides still in force at `now_ms`, oldest first.
    pub fn overrides(&self, project: &str, now_ms: u64) -> Result<Vec<OverrideRow>, StoreError> {
        let p = project.to_string();
        self.read(move |c| {
            let mut st = c.prepare(
                "SELECT id, project, key, value, owner, created_at, expires_at FROM overrides
                 WHERE project = ?1 AND expires_at > ?2 ORDER BY id",
            )?;
            let rows = st.query_map(params![p, now_ms as i64], |r| {
                Ok(OverrideRow {
                    id: r.get(0)?,
                    project: r.get(1)?,
                    key: r.get(2)?,
                    value: r.get(3)?,
                    owner: r.get(4)?,
                    created_at: r.get::<_, i64>(5)? as u64,
                    expires_at: r.get::<_, i64>(6)? as u64,
                })
            })?;
            rows.collect()
        })
    }

    /// Delete overrides that have expired. Returns how many.
    pub fn expire_overrides(&self, now_ms: u64) -> Result<usize, StoreError> {
        self.write(move |c| {
            c.execute(
                "DELETE FROM overrides WHERE expires_at <= ?1",
                params![now_ms as i64],
            )
        })
    }

    /// An override for one task only (a model choice, a skip, ...).
    pub fn set_task_override(
        &self,
        task: u64,
        key: &str,
        value: &str,
        owner: &str,
        now_ms: u64,
    ) -> Result<(), StoreError> {
        let (k, v, o) = (key.to_string(), value.to_string(), owner.to_string());
        self.write(move |c| {
            c.execute(
                "INSERT OR REPLACE INTO task_overrides(task, key, value, owner, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![task as i64, k, v, o, now_ms as i64],
            )
            .map(|_| ())
        })
    }

    pub fn task_overrides(&self, task: u64) -> Result<Vec<(String, String)>, StoreError> {
        self.read(move |c| {
            let mut st =
                c.prepare("SELECT key, value FROM task_overrides WHERE task = ?1 ORDER BY key")?;
            let rows = st.query_map(params![task as i64], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect()
        })
    }

    pub fn clear_task_override(&self, task: u64, key: &str) -> Result<usize, StoreError> {
        let k = key.to_string();
        self.write(move |c| {
            c.execute(
                "DELETE FROM task_overrides WHERE task = ?1 AND key = ?2",
                params![task as i64, k],
            )
        })
    }

    // ---- attention and overrides (Stage 10) ----

    #[allow(clippy::too_many_arguments)]
    pub fn record_intervention(
        &self,
        kind: &str,
        task: Option<u64>,
        project: &str,
        who: &str,
        detail: &str,
        reason: &str,
        now_ms: u64,
    ) -> Result<u64, StoreError> {
        let (k, p, w, d, r) = (
            kind.to_string(),
            project.to_string(),
            who.to_string(),
            detail.to_string(),
            reason.to_string(),
        );
        self.write(move |c| {
            c.execute(
                "INSERT INTO interventions(at, kind, task, project, who, detail, reason) VALUES (?1,?2,?3,?4,?5,?6,?7)",
                params![now_ms as i64, k, task.map(|t| t as i64), p, w, d, r],
            )?;
            Ok(c.last_insert_rowid() as u64)
        })
    }

    /// Interventions since `since_ms`, oldest first.
    pub fn interventions(&self, since_ms: u64) -> Result<Vec<InterventionRow>, StoreError> {
        self.read(move |c| {
            let mut st = c.prepare(
                "SELECT id, at, kind, task, project, who, detail, reason FROM interventions WHERE at >= ?1 ORDER BY id",
            )?;
            let rows = st.query_map(params![since_ms as i64], |r| {
                Ok(InterventionRow {
                    id: r.get::<_, i64>(0)? as u64,
                    at: r.get::<_, i64>(1)? as u64,
                    kind: r.get(2)?,
                    task: r.get::<_, Option<i64>>(3)?.map(|t| t as u64),
                    project: r.get(4)?,
                    who: r.get(5)?,
                    detail: r.get(6)?,
                    reason: r.get(7)?,
                })
            })?;
            rows.collect()
        })
    }

    pub fn add_pause(
        &self,
        project: &str,
        mode: &str,
        owner: &str,
        now_ms: u64,
        expires_at_ms: u64,
    ) -> Result<u64, StoreError> {
        let (p, m, o) = (project.to_string(), mode.to_string(), owner.to_string());
        self.write(move |c| {
            c.execute(
                "INSERT INTO pauses(project, mode, owner, created_at, expires_at) VALUES (?1,?2,?3,?4,?5)",
                params![p, m, o, now_ms as i64, expires_at_ms as i64],
            )?;
            Ok(c.last_insert_rowid() as u64)
        })
    }

    /// Lift pauses: those of `project` (empty: the global ones) in `mode`
    /// (empty: any). Returns how many.
    pub fn remove_pauses(&self, project: &str, mode: &str) -> Result<usize, StoreError> {
        let (p, m) = (project.to_string(), mode.to_string());
        self.write(move |c| {
            c.execute(
                "DELETE FROM pauses WHERE project = ?1 AND (?2 = '' OR mode = ?2)",
                params![p, m],
            )
        })
    }

    /// Pauses still in force at `now_ms`.
    pub fn pauses(&self, now_ms: u64) -> Result<Vec<PauseRow>, StoreError> {
        self.read(move |c| {
            let mut st = c.prepare(
                "SELECT id, project, mode, owner, created_at, expires_at FROM pauses WHERE expires_at > ?1 ORDER BY id",
            )?;
            let rows = st.query_map(params![now_ms as i64], |r| {
                Ok(PauseRow {
                    id: r.get::<_, i64>(0)? as u64,
                    project: r.get(1)?,
                    mode: r.get(2)?,
                    owner: r.get(3)?,
                    created_at: r.get::<_, i64>(4)? as u64,
                    expires_at: r.get::<_, i64>(5)? as u64,
                })
            })?;
            rows.collect()
        })
    }

    /// Make the alert table match the conditions that are true now: raise the
    /// new ones, keep the ones still true, clear the ones that went away.
    /// `now` is `(key, kind, text)`. Returns `(raised, cleared)` keys.
    #[allow(clippy::type_complexity)]
    pub fn sync_alerts(
        &self,
        now: &[(String, String, String)],
        now_ms: u64,
    ) -> Result<(Vec<String>, Vec<String>), StoreError> {
        let now = now.to_vec();
        self.write(move |c| {
            let mut raised = vec![];
            let mut cleared = vec![];
            let active: Vec<String> = {
                let mut st = c.prepare("SELECT key FROM alerts WHERE cleared_at IS NULL")?;
                let rows = st.query_map([], |r| r.get::<_, String>(0))?;
                rows.collect::<Result<_, _>>()?
            };
            for (key, kind, text) in &now {
                if active.contains(key) {
                    c.execute("UPDATE alerts SET text = ?2 WHERE key = ?1", params![key, text])?;
                } else {
                    c.execute(
                        "INSERT OR REPLACE INTO alerts(key, kind, text, since, cleared_at) VALUES (?1,?2,?3,?4,NULL)",
                        params![key, kind, text, now_ms as i64],
                    )?;
                    raised.push(key.clone());
                }
            }
            for key in active {
                if !now.iter().any(|(k, _, _)| *k == key) {
                    c.execute(
                        "UPDATE alerts SET cleared_at = ?2 WHERE key = ?1",
                        params![key, now_ms as i64],
                    )?;
                    cleared.push(key);
                }
            }
            Ok((raised, cleared))
        })
    }

    pub fn alerts(&self, active_only: bool) -> Result<Vec<AlertRow>, StoreError> {
        self.read(move |c| {
            let mut st = c.prepare(
                "SELECT key, kind, text, since, cleared_at FROM alerts WHERE (?1 = 0 OR cleared_at IS NULL) ORDER BY since, key",
            )?;
            let rows = st.query_map(params![i64::from(active_only)], |r| {
                Ok(AlertRow {
                    key: r.get(0)?,
                    kind: r.get(1)?,
                    text: r.get(2)?,
                    since: r.get::<_, i64>(3)? as u64,
                    cleared_at: r.get::<_, Option<i64>>(4)?.map(|t| t as u64),
                })
            })?;
            rows.collect()
        })
    }

    // ---- the testing system's memory (Stage 9) ----

    /// Remember a green result for this exact tree and suite.
    pub fn put_evidence(
        &self,
        tree: &str,
        suite: &str,
        executed: usize,
        passed: usize,
        task: u64,
        now_ms: u64,
    ) -> Result<(), StoreError> {
        let (t, s) = (tree.to_string(), suite.to_string());
        self.write(move |c| {
            c.execute(
                "INSERT OR REPLACE INTO evidence(tree, suite, executed, passed, at, task) VALUES (?1,?2,?3,?4,?5,?6)",
                params![t, s, executed as i64, passed as i64, now_ms as i64, task as i64],
            )
            .map(|_| ())
        })
    }

    /// `(executed, passed, task)` if this tree already passed this suite.
    pub fn evidence(
        &self,
        tree: &str,
        suite: &str,
    ) -> Result<Option<(usize, usize, u64)>, StoreError> {
        let (t, s) = (tree.to_string(), suite.to_string());
        self.read(move |c| {
            c.query_row(
                "SELECT executed, passed, task FROM evidence WHERE tree = ?1 AND suite = ?2",
                params![t, s],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)? as usize,
                        r.get::<_, i64>(1)? as usize,
                        r.get::<_, i64>(2)? as u64,
                    ))
                },
            )
            .optional()
        })
    }

    /// What fails at a base commit, cached so it is only worked out once.
    pub fn put_baseline(
        &self,
        base: &str,
        suite: &str,
        failing: &[String],
        names: &[String],
        executed: usize,
        now_ms: u64,
    ) -> Result<(), StoreError> {
        let (b, s, f, n) = (
            base.to_string(),
            suite.to_string(),
            failing.join("\n"),
            names.join("\n"),
        );
        self.write(move |c| {
            c.execute(
                "INSERT OR REPLACE INTO baselines(base, suite, failing, names, executed, at) VALUES (?1,?2,?3,?4,?5,?6)",
                params![b, s, f, n, executed as i64, now_ms as i64],
            )
            .map(|_| ())
        })
    }

    /// `(failing, all test names)` at a base commit, if it was worked out.
    #[allow(clippy::type_complexity)]
    pub fn baseline(
        &self,
        base: &str,
        suite: &str,
    ) -> Result<Option<(Vec<String>, Vec<String>)>, StoreError> {
        let (b, s) = (base.to_string(), suite.to_string());
        self.read(move |c| {
            c.query_row(
                "SELECT failing, names FROM baselines WHERE base = ?1 AND suite = ?2",
                params![b, s],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()
            .map(|o| {
                o.map(|(f, n)| {
                    (
                        f.lines().map(String::from).collect(),
                        n.lines().map(String::from).collect(),
                    )
                })
            })
        })
    }

    pub fn record_flaky(
        &self,
        suite: &str,
        test: &str,
        task: u64,
        now_ms: u64,
    ) -> Result<(), StoreError> {
        let (s, t) = (suite.to_string(), test.to_string());
        self.write(move |c| {
            c.execute(
                "INSERT INTO flaky(at, suite, test, task) VALUES (?1,?2,?3,?4)",
                params![now_ms as i64, s, t, task as i64],
            )
            .map(|_| ())
        })
    }

    /// `(suite, test, at_ms)` for every flake since `since_ms`.
    pub fn flaky_since(&self, since_ms: u64) -> Result<Vec<(String, String, u64)>, StoreError> {
        self.read(move |c| {
            let mut st =
                c.prepare("SELECT suite, test, at FROM flaky WHERE at >= ?1 ORDER BY id")?;
            let rows = st.query_map(params![since_ms as i64], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? as u64))
            })?;
            rows.collect()
        })
    }

    /// Record a debt: something skipped by force. `scope` is `suite:NAME` or
    /// `step:NAME`. Returns its id.
    #[allow(clippy::too_many_arguments)]
    pub fn add_debt(
        &self,
        task: u64,
        project: &str,
        scope: &str,
        reason: &str,
        owner: &str,
        deferred: bool,
        now_ms: u64,
    ) -> Result<u64, StoreError> {
        let (p, s, r, o) = (
            project.to_string(),
            scope.to_string(),
            reason.to_string(),
            owner.to_string(),
        );
        self.write(move |c| {
            c.execute(
                "INSERT INTO debts(at, task, project, scope, reason, owner, deferred, status) VALUES (?1,?2,?3,?4,?5,?6,?7,'open')",
                params![now_ms as i64, task as i64, p, s, r, o, i64::from(deferred)],
            )?;
            Ok(c.last_insert_rowid() as u64)
        })
    }

    /// `(id, at, task, project, scope, reason, owner, deferred, status)`.
    #[allow(clippy::type_complexity)]
    pub fn debts(&self, task: Option<u64>, open_only: bool) -> Result<Vec<DebtRow>, StoreError> {
        self.read(move |c| {
            let mut st = c.prepare(
                "SELECT id, at, task, project, scope, reason, owner, deferred, status FROM debts
                 WHERE (?1 IS NULL OR task = ?1) AND (?2 = 0 OR status = 'open') ORDER BY id",
            )?;
            let rows =
                st.query_map(params![task.map(|t| t as i64), i64::from(open_only)], |r| {
                    Ok(DebtRow {
                        id: r.get::<_, i64>(0)? as u64,
                        at: r.get::<_, i64>(1)? as u64,
                        task: r.get::<_, i64>(2)? as u64,
                        project: r.get(3)?,
                        scope: r.get(4)?,
                        reason: r.get(5)?,
                        owner: r.get(6)?,
                        deferred: r.get::<_, i64>(7)? != 0,
                        status: r.get(8)?,
                    })
                })?;
            rows.collect()
        })
    }

    // ---- landing (Stage 14) ----

    pub fn add_batch(&self, project: &str, tasks: &str, now_ms: u64) -> Result<u64, StoreError> {
        let (p, t) = (project.to_string(), tasks.to_string());
        self.write(move |c| {
            c.execute(
                "INSERT INTO batches(project, at, phase, tasks) VALUES (?1,?2,'forming',?3)",
                params![p, now_ms as i64, t],
            )?;
            Ok(c.last_insert_rowid() as u64)
        })
    }

    /// Move a batch to a phase and record what is known so far. `finished`
    /// stamps the end.
    #[allow(clippy::too_many_arguments)]
    pub fn set_batch(
        &self,
        id: u64,
        phase: &str,
        landed: &str,
        parked: &str,
        note: &str,
        head: &str,
        finished: bool,
        now_ms: u64,
    ) -> Result<(), StoreError> {
        let (ph, l, pk, n, h) = (
            phase.to_string(),
            landed.to_string(),
            parked.to_string(),
            note.to_string(),
            head.to_string(),
        );
        self.write(move |c| {
            c.execute(
                "UPDATE batches SET phase=?2, landed=?3, parked=?4, note=?5, head=?6,
                 finished_at = CASE WHEN ?7 THEN ?8 ELSE finished_at END WHERE id=?1",
                params![id as i64, ph, l, pk, n, h, finished, now_ms as i64],
            )
            .map(|_| ())
        })
    }

    /// Newest first.
    pub fn batches(&self, project: Option<&str>, limit: u32) -> Result<Vec<BatchRow>, StoreError> {
        let p = project.map(String::from);
        self.read(move |c| {
            let mut st = c.prepare(
                "SELECT id, project, at, finished_at, phase, tasks, landed, parked, note, head
                 FROM batches WHERE (?1 IS NULL OR project = ?1) ORDER BY id DESC LIMIT ?2",
            )?;
            let rows = st.query_map(params![p, limit], |r| {
                Ok(BatchRow {
                    id: r.get::<_, i64>(0)? as u64,
                    project: r.get(1)?,
                    at: r.get::<_, i64>(2)? as u64,
                    finished_at: r.get::<_, Option<i64>>(3)?.map(|t| t as u64),
                    phase: r.get(4)?,
                    tasks: r.get(5)?,
                    landed: r.get(6)?,
                    parked: r.get(7)?,
                    note: r.get(8)?,
                    head: r.get(9)?,
                })
            })?;
            rows.collect()
        })
    }

    /// Batches a dead daemon left unfinished.
    pub fn abandon_batches(&self, project: &str, now_ms: u64) -> Result<usize, StoreError> {
        let p = project.to_string();
        self.write(move |c| {
            c.execute(
                "UPDATE batches SET phase='abandoned', note='the daemon restarted; its tasks were queued again', finished_at=?2
                 WHERE project=?1 AND finished_at IS NULL",
                params![p, now_ms as i64],
            )
        })
    }

    pub fn add_ref(
        &self,
        batch: u64,
        project: &str,
        kind: &str,
        name: &str,
        number: Option<u64>,
        now_ms: u64,
    ) -> Result<u64, StoreError> {
        let (p, k, n) = (project.to_string(), kind.to_string(), name.to_string());
        self.write(move |c| {
            c.execute(
                "INSERT INTO landing_refs(batch, project, kind, name, number, at) VALUES (?1,?2,?3,?4,?5,?6)",
                params![batch as i64, p, k, n, number.map(|x| x as i64), now_ms as i64],
            )?;
            Ok(c.last_insert_rowid() as u64)
        })
    }

    pub fn set_ref_number(&self, id: u64, number: u64) -> Result<(), StoreError> {
        self.write(move |c| {
            c.execute(
                "UPDATE landing_refs SET number = ?2 WHERE id = ?1",
                params![id as i64, number as i64],
            )
            .map(|_| ())
        })
    }

    pub fn remove_ref(&self, id: u64) -> Result<(), StoreError> {
        self.write(move |c| {
            c.execute(
                "UPDATE landing_refs SET removed = 1 WHERE id = ?1",
                params![id as i64],
            )
            .map(|_| ())
        })
    }

    /// Refs and PRs not yet cleaned up, of a project.
    pub fn open_refs(&self, project: &str) -> Result<Vec<RefRow>, StoreError> {
        let p = project.to_string();
        self.read(move |c| {
            let mut st = c.prepare(
                "SELECT id, batch, project, kind, name, number FROM landing_refs WHERE project = ?1 AND removed = 0 ORDER BY id",
            )?;
            let rows = st.query_map(params![p], |r| {
                Ok(RefRow {
                    id: r.get::<_, i64>(0)? as u64,
                    batch: r.get::<_, i64>(1)? as u64,
                    project: r.get(2)?,
                    kind: r.get(3)?,
                    name: r.get(4)?,
                    number: r.get::<_, Option<i64>>(5)?.map(|x| x as u64),
                })
            })?;
            rows.collect()
        })
    }

    pub fn put_tracker_item(&self, row: &TrackerItemRow) -> Result<(), StoreError> {
        let r = row.clone();
        self.write(move |c| {
            c.execute(
                "INSERT OR REPLACE INTO tracker_items(project, number, task, title, body, priority, open, mirrored) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                params![r.project, r.number as i64, r.task as i64, r.title, r.body, r.priority, r.open, r.mirrored],
            )
            .map(|_| ())
        })
    }

    pub fn tracker_items(&self, project: &str) -> Result<Vec<TrackerItemRow>, StoreError> {
        let p = project.to_string();
        self.read(move |c| {
            let mut st = c.prepare(
                "SELECT project, number, task, title, body, priority, open, mirrored FROM tracker_items WHERE project = ?1 ORDER BY number",
            )?;
            let rows = st.query_map(params![p], |r| {
                Ok(TrackerItemRow {
                    project: r.get(0)?,
                    number: r.get::<_, i64>(1)? as u64,
                    task: r.get::<_, i64>(2)? as u64,
                    title: r.get(3)?,
                    body: r.get(4)?,
                    priority: r.get(5)?,
                    open: r.get::<_, i64>(6)? != 0,
                    mirrored: r.get(7)?,
                })
            })?;
            rows.collect()
        })
    }

    pub fn set_freeze(
        &self,
        project: &str,
        reason: &str,
        fix_task: Option<u64>,
        now_ms: u64,
    ) -> Result<(), StoreError> {
        let (p, r) = (project.to_string(), reason.to_string());
        self.write(move |c| {
            c.execute(
                "INSERT OR REPLACE INTO freezes(project, since, reason, fix_task) VALUES (?1,?2,?3,?4)",
                params![p, now_ms as i64, r, fix_task.map(|t| t as i64)],
            )
            .map(|_| ())
        })
    }

    pub fn clear_freeze(&self, project: &str) -> Result<bool, StoreError> {
        let p = project.to_string();
        self.write(move |c| {
            c.execute("DELETE FROM freezes WHERE project = ?1", params![p])
                .map(|n| n > 0)
        })
    }

    pub fn freezes(&self) -> Result<Vec<FreezeRow>, StoreError> {
        self.read(move |c| {
            let mut st =
                c.prepare("SELECT project, since, reason, fix_task FROM freezes ORDER BY project")?;
            let rows = st.query_map([], |r| {
                Ok(FreezeRow {
                    project: r.get(0)?,
                    since: r.get::<_, i64>(1)? as u64,
                    reason: r.get(2)?,
                    fix_task: r.get::<_, Option<i64>>(3)?.map(|t| t as u64),
                })
            })?;
            rows.collect()
        })
    }

    /// Mark a task `now` (its own batch, ahead of the window) or `force`
    /// (no verification, past a freeze).
    pub fn set_landing_flag(
        &self,
        task: u64,
        kind: &str,
        owner: &str,
        now_ms: u64,
    ) -> Result<(), StoreError> {
        let (k, o) = (kind.to_string(), owner.to_string());
        self.write(move |c| {
            c.execute(
                "INSERT OR REPLACE INTO landing_flags(task, kind, at, owner) VALUES (?1,?2,?3,?4)",
                params![task as i64, k, now_ms as i64, o],
            )
            .map(|_| ())
        })
    }

    pub fn landing_flag(&self, task: u64) -> Result<Option<String>, StoreError> {
        self.read(move |c| {
            c.query_row(
                "SELECT kind FROM landing_flags WHERE task = ?1",
                params![task as i64],
                |r| r.get(0),
            )
            .optional()
        })
    }

    pub fn clear_landing_flag(&self, task: u64) -> Result<(), StoreError> {
        self.write(move |c| {
            c.execute(
                "DELETE FROM landing_flags WHERE task = ?1",
                params![task as i64],
            )
            .map(|_| ())
        })
    }

    pub fn pay_debt(&self, id: u64, now_ms: u64) -> Result<bool, StoreError> {
        self.write(move |c| {
            c.execute(
                "UPDATE debts SET status = 'paid', paid_at = ?2 WHERE id = ?1 AND status = 'open'",
                params![id as i64, now_ms as i64],
            )
            .map(|n| n > 0)
        })
    }

    pub fn put_quarantine(
        &self,
        suite: &str,
        test: &str,
        owner: &str,
        expires_at_ms: u64,
        reason: &str,
        now_ms: u64,
    ) -> Result<(), StoreError> {
        let (s, t, o, r) = (
            suite.to_string(),
            test.to_string(),
            owner.to_string(),
            reason.to_string(),
        );
        self.write(move |c| {
            c.execute(
                "INSERT OR REPLACE INTO quarantine(suite, test, owner, expires_at, reason, at) VALUES (?1,?2,?3,?4,?5,?6)",
                params![s, t, o, expires_at_ms as i64, r, now_ms as i64],
            )
            .map(|_| ())
        })
    }

    pub fn remove_quarantine(&self, suite: &str, test: &str) -> Result<usize, StoreError> {
        let (s, t) = (suite.to_string(), test.to_string());
        self.write(move |c| {
            c.execute(
                "DELETE FROM quarantine WHERE suite = ?1 AND test = ?2",
                params![s, t],
            )
        })
    }

    /// `(suite, test, owner, expires_at_ms, reason)` still in force at `now_ms`.
    #[allow(clippy::type_complexity)]
    pub fn quarantined(
        &self,
        now_ms: u64,
    ) -> Result<Vec<(String, String, String, u64, String)>, StoreError> {
        self.read(move |c| {
            let mut st = c.prepare(
                "SELECT suite, test, owner, expires_at, reason FROM quarantine WHERE expires_at > ?1 ORDER BY suite, test",
            )?;
            let rows = st.query_map(params![now_ms as i64], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get::<_, i64>(3)? as u64, r.get(4)?))
            })?;
            rows.collect()
        })
    }

    pub fn set_closing(
        &self,
        task: u64,
        verdict: &str,
        note: &str,
        now_ms: u64,
    ) -> Result<(), StoreError> {
        let (v, n) = (verdict.to_string(), note.to_string());
        self.write(move |c| {
            c.execute(
                "INSERT OR REPLACE INTO closings(task, verdict, note, at) VALUES (?1,?2,?3,?4)",
                params![task as i64, v, n, now_ms as i64],
            )
            .map(|_| ())
        })
    }

    /// `(verdict, note)`.
    pub fn closing(&self, task: u64) -> Result<Option<(String, String)>, StoreError> {
        self.read(move |c| {
            c.query_row(
                "SELECT verdict, note FROM closings WHERE task = ?1",
                params![task as i64],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
        })
    }

    /// File a bug: the reconcile sweep found something the event path missed.
    pub fn record_bug(&self, at_ms: u64, kind: &str, detail: &str) -> Result<(), StoreError> {
        let (rtx, rrx) = channel();
        self.tx
            .send(Cmd::Bug(at_ms, kind.into(), detail.into(), rtx))
            .map_err(|_| StoreError::WriterGone)?;
        rrx.recv().map_err(|_| StoreError::WriterGone)?
    }

    /// Bugs filed so far: `(at_ms, kind, detail)`, oldest first.
    pub fn bugs(&self) -> Result<Vec<(u64, String, String)>, StoreError> {
        let conn = self.reader.lock().expect("reader lock");
        let mut st = conn.prepare("SELECT at, kind, detail FROM bugs ORDER BY id")?;
        let rows = st.query_map([], |r| {
            Ok((r.get::<_, i64>(0)? as u64, r.get(1)?, r.get(2)?))
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    /// The projection tables as sorted text lines, for tests and diagnostics.
    pub fn projection_dump(&self) -> Result<Vec<String>, StoreError> {
        let conn = self.reader.lock().expect("reader lock");
        dump_db(&conn)
    }

    /// Check that the log is contiguous and replays cleanly, and that the
    /// projections match that replay exactly (nothing half-applied).
    pub fn verify(&self) -> Result<(), StoreError> {
        let conn = self.reader.lock().expect("reader lock");
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM events", [], |r| r.get(0))?;
        let last = last_seq(&conn)?;
        if n as u64 != last {
            return Err(StoreError::Inconsistent(format!(
                "{n} events but last seq {last}: gap"
            )));
        }
        let projected = projected_seq(&conn)?;
        if projected != last {
            return Err(StoreError::Inconsistent(format!(
                "projected {projected}, log at {last}"
            )));
        }
        let world = replay(&conn)?;
        let (want, got) = (dump_world(&world), dump_db(&conn)?);
        if want != got {
            return Err(StoreError::Inconsistent(format!(
                "projection differs from replay:\n want {want:?}\n got  {got:?}"
            )));
        }
        Ok(())
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Shutdown);
        if let Some(h) = self.writer.take() {
            let _ = h.join();
        }
    }
}

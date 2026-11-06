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

pub use codec::CodecError;

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Appended {
    pub seq: u64,
    pub effects: Vec<Effect>,
}

enum Cmd {
    Append(Event, Sender<Result<Appended, StoreError>>),
    World(Sender<World>),
    Rebuild(Sender<Result<World, StoreError>>),
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
        "task {}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
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
                enqueued_at, failures, decision FROM tasks",
    )?;
    let rows = st.query_map([], |r| {
        let dec: Option<i64> = r.get(9)?;
        Ok(format!(
            "task {}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
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
                                      priority, enqueued_at, failures, decision)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
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

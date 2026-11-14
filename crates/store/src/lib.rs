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
pub mod devices;
pub mod introspect;
pub mod migrations;
pub mod notify;

use mason_core::*;
use mason_engine::{reduce_in_place, ReduceError, Touched, World};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;
use std::sync::Mutex;
use std::thread::JoinHandle;

pub use codec::{decode as decode_event, encode as encode_event, state_name, CodecError};
pub use devices::DeviceRow;
pub use introspect::{FindingRow, ScheduleRow, TriageRow};
pub use notify::{AnnouncementRow, DeliveryRow};

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

/// What a suite did at a base commit (s246): failing names and every name, the executed count,
/// and why a base can be called red the same way as a branch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BaseProfile {
    pub failing: Vec<String>,
    pub names: Vec<String>,
    pub executed: usize,
    /// The build was red: its first error.
    pub build_error: Option<String>,
    /// The run executed too few tests to prove anything.
    pub vacuous: bool,
    /// The run was red.
    pub red: bool,
    /// The first error line of a red run.
    pub first_error: String,
}

/// A task waiting in its test step because the base is red the same way (s246).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BaseRedWait {
    pub task: u64,
    pub suite: String,
    /// `build`, `vacuous` or `nameless`.
    pub kind: String,
    pub first_error: String,
    /// The base commit the task was last found red against.
    pub base: String,
    pub text: String,
    pub at_ms: u64,
}

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

/// One window reading kept for the trend. Times are epoch seconds.
#[derive(Clone, Debug, PartialEq)]
pub struct UsageReading {
    pub account: String,
    pub window: String,
    pub at: u64,
    pub utilization: Option<f64>,
    pub resets_at: Option<u64>,
    pub state: String,
}

/// An account held back as a whole. Times are epoch milliseconds.
#[derive(Clone, Debug, PartialEq)]
pub struct AccountHold {
    pub account: String,
    /// limit | outage | overage | budget | manual
    pub kind: String,
    pub why: String,
    pub since_ms: u64,
    /// The provider's reset, when it gave one that could be read.
    pub until_ms: Option<u64>,
    /// The current backoff when there is no reset.
    pub backoff_ms: u64,
    /// When the next probe session may start (no reset known).
    pub probe_ms: u64,
    pub by: String,
}

/// The tokens and cost one step attempt reported. `None` means not reported.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UsageRun {
    pub attempt: String,
    pub task: u64,
    pub project: String,
    pub step: String,
    pub account: String,
    pub model: Option<String>,
    pub input: Option<u64>,
    pub output: Option<u64>,
    pub cache_read: Option<u64>,
    pub cache_create: Option<u64>,
    pub cost_usd: Option<f64>,
    pub at: u64,
}

/// One reading per window per account is kept per this many seconds, unless
/// the window's state or reset changed.
pub const READING_GAP_SECS: u64 = 300;
/// Readings older than this are dropped on the next write.
pub const READING_KEEP_SECS: u64 = 8 * 86400;

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
    /// The owner has seen it: its noise stops, it stays visible while the condition holds.
    pub acked_at: Option<u64>,
    pub reminded_at: Option<u64>,
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
    /// Whether the issue carried the project's label at the last look (`None`: not looked
    /// at since this was recorded). Decision 0131.
    pub labelled: Option<bool>,
}

/// A self-change waiting for (or given) the owner's approval.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HoldRow {
    pub task: u64,
    pub diff_hash: String,
    pub reasons: String,
    pub at: u64,
    pub approved_by: Option<String>,
    pub approved_at: Option<u64>,
}

/// A proposal: a task plus what the agent saw and where it was routed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProposalRow {
    pub task: u64,
    pub project: String,
    /// `agent`, `outside` or `owner`.
    pub source: String,
    /// `normal` or `blocks`.
    pub severity: String,
    /// `project` or `pipeline`.
    pub route: String,
    /// The task whose agent filed it.
    pub filed_by: Option<u64>,
    pub title: String,
    pub body: String,
    pub evidence: String,
    pub key: String,
    /// `open`, `approved` or `rejected`.
    pub status: String,
    pub reject_reason: String,
    /// `local`, `previewed` or `published`.
    pub publish_state: String,
    /// For an improvement: `write` or `fork`. Empty for the rest.
    pub access: String,
    pub branch: String,
    pub pr: Option<u64>,
    /// `open`, `merged`, `closed` or `conflict` once a pull request exists.
    pub pr_state: String,
    pub at: u64,
}

/// A number MASON reserved for a task: a decision record, or an exit script / stage note.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReservationRow {
    pub task: u64,
    /// `decision` or `exit`.
    pub kind: String,
    pub number: u32,
    /// The task was cancelled: the number is not reused.
    pub released: bool,
    pub at: u64,
}

/// A project's landing freeze: its post-land tier is red, so only the fix may land.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FreezeRow {
    pub project: String,
    pub since: u64,
    pub reason: String,
    pub fix_task: Option<u64>,
    /// Who set it: `post-land` or `ci` (the base-branch CI watch).
    pub source: String,
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

/// A step host that was lost without an exit record, with what was known about it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostLossRow {
    pub host_id: String,
    pub task: u64,
    pub run: u64,
    pub step: String,
    pub lost_at_ms: u64,
    /// The report text: "host pid 1234 gone at 16:16:53, killed by signal 9, ...".
    pub detail: String,
    /// The step was run again on a fresh host (false: this loss failed the step).
    pub retried: bool,
}

/// One session start of a step run: what was asked for and what was launched.
/// Empty strings mean "none" (no tier, the CLI's own default model, no effort).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LaunchRow {
    pub run: u64,
    pub seq: u64,
    pub task: u64,
    pub step: String,
    /// `launch`, `retry` or `reattach`.
    pub kind: String,
    pub tier: String,
    pub level: String,
    /// What the mapping alone asked for.
    pub req_model: String,
    pub req_effort: String,
    /// What was passed to the CLI.
    pub model: String,
    pub effort: String,
    pub provider: String,
    pub model_from: String,
    pub effort_from: String,
    pub retry_n: u32,
    pub retry_max: u32,
    pub moved_up_from: String,
    pub overridden: bool,
    /// The model the CLI said it used, when it said.
    pub reported_model: String,
    pub at_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Appended {
    pub seq: u64,
    pub effects: Vec<Effect>,
}

enum Cmd {
    Append(Event, Sender<Result<Appended, StoreError>>),
    World(Sender<Arc<World>>),
    Rebuild(Sender<Result<Arc<World>, StoreError>>),
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
            // In place: an error here is fatal, so there is nothing to roll back.
            reduce_in_place(&mut world, &se.event).map_err(|e| StoreError::Corrupt {
                seq: se.seq,
                why: format!("{e:?}"),
            })?;
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

/// Write every projection row for `w`.
fn write_all(conn: &Connection, w: &World) -> rusqlite::Result<()> {
    for p in w.projects.values() {
        write_project(conn, p)?;
    }
    for t in w.tasks.values() {
        write_task(conn, t)?;
    }
    for d in w.decisions.values() {
        write_decision(conn, d)?;
    }
    Ok(())
}

/// Write the rows an event touched, as they now are in `w`.
fn write_touched(conn: &Connection, w: &World, t: &Touched) -> rusqlite::Result<()> {
    for p in t.projects.iter().filter_map(|id| w.projects.get(id)) {
        write_project(conn, p)?;
    }
    for x in t.tasks.iter().filter_map(|id| w.tasks.get(id)) {
        write_task(conn, x)?;
    }
    for d in t.decisions.iter().filter_map(|id| w.decisions.get(id)) {
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
fn rebuild(conn: &mut Connection) -> Result<Arc<World>, StoreError> {
    let tx = conn.transaction()?;
    let world = replay(&tx)?;
    tx.execute_batch("DELETE FROM decisions; DELETE FROM tasks; DELETE FROM projects;")?;
    write_all(&tx, &world)?;
    set_projected(&tx, last_seq(&tx)?)?;
    tx.commit()?;
    Ok(Arc::new(world))
}

/// The writer thread. A panic here must never leave the store half alive (every append and
/// every `world()` would fail for the life of the process while the daemon still answers),
/// so it ends the process: launchd restarts it, recovery runs, and a build that keeps doing
/// this is rolled back by the existing rule.
fn writer_loop(conn: Connection, world: Arc<World>, rx: std::sync::mpsc::Receiver<Cmd>) {
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| writer_run(conn, world, rx)));
    if let Err(e) = r {
        let what = e
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| e.downcast_ref::<String>().cloned())
            .unwrap_or_default();
        eprintln!(
            "FATAL: the store writer thread panicked ({what}); exiting so the daemon is restarted"
        );
        std::process::exit(WRITER_PANIC_EXIT);
    }
}

/// The exit status when the writer thread panics (EX_SOFTWARE).
pub const WRITER_PANIC_EXIT: i32 = 70;

fn writer_run(mut conn: Connection, mut world: Arc<World>, rx: std::sync::mpsc::Receiver<Cmd>) {
    while let Ok(cmd) = rx.recv() {
        match cmd {
            Cmd::Shutdown => break,
            Cmd::World(reply) => {
                let _ = reply.send(Arc::clone(&world));
            }
            Cmd::Rebuild(reply) => {
                let r = rebuild(&mut conn);
                if let Ok(w) = &r {
                    world = Arc::clone(w);
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
            Cmd::Append(ev, reply) => {
                let _ = reply.send(append_one(&mut conn, &mut world, &ev));
            }
        }
    }
}

/// Apply `ev` to the writer's world in place (`Arc::make_mut` copies only when a reader
/// still holds a snapshot) and persist it. A refused event changes nothing. If the
/// transaction fails after the world changed, the world is rebuilt from the log, which
/// is rare and correct; a failure of that too ends the process (see `writer_loop`).
///
/// Chosen over cheap-to-clone `Task`/`StepRun` because it removes the copy altogether
/// instead of making it cheaper: an append costs the touched rows, not a walk of every task.
fn append_one(
    conn: &mut Connection,
    world: &mut Arc<World>,
    ev: &Event,
) -> Result<Appended, StoreError> {
    let (effects, touched) =
        reduce_in_place(Arc::make_mut(world), ev).map_err(StoreError::Rejected)?;
    match persist(conn, world, ev, &touched) {
        Ok(seq) => Ok(Appended { seq, effects }),
        Err(e) => {
            *world = Arc::new(replay(conn).expect("replay after a failed append"));
            Err(e)
        }
    }
}

fn persist(
    conn: &mut Connection,
    world: &World,
    ev: &Event,
    touched: &Touched,
) -> Result<u64, StoreError> {
    let (kind, payload) = codec::encode(&ev.kind);
    let tx = conn.transaction()?;
    tx.execute(
        "INSERT INTO events(at, kind, payload) VALUES (?1, ?2, ?3)",
        params![ev.at.0 as i64, kind, payload],
    )?;
    let seq = tx.last_insert_rowid() as u64;
    write_touched(&tx, world, touched)?;
    set_projected(&tx, seq)?;
    tx.commit()?;
    Ok(seq)
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
            Arc::new(replay(&conn)?)
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
        let conn = self.reader.lock().unwrap_or_else(|e| e.into_inner());
        read_events(&conn, after, limit)
    }

    pub fn last_seq(&self) -> Result<u64, StoreError> {
        let conn = self.reader.lock().unwrap_or_else(|e| e.into_inner());
        last_seq(&conn)
    }

    /// The current in-memory world, as the writer sees it.
    pub fn world(&self) -> Result<Arc<World>, StoreError> {
        let (rtx, rrx) = channel();
        self.tx
            .send(Cmd::World(rtx))
            .map_err(|_| StoreError::WriterGone)?;
        rrx.recv().map_err(|_| StoreError::WriterGone)
    }

    /// Rebuild every projection from the log.
    pub fn rebuild_projections(&self) -> Result<Arc<World>, StoreError> {
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
        let conn = self.reader.lock().unwrap_or_else(|e| e.into_inner());
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

    /// Lift the pauses of `project` in `mode` that `owner` made. Returns how many.
    pub fn remove_pauses_of(
        &self,
        project: &str,
        mode: &str,
        owner: &str,
    ) -> Result<usize, StoreError> {
        let (p, m, o) = (project.to_string(), mode.to_string(), owner.to_string());
        self.write(move |c| {
            c.execute(
                "DELETE FROM pauses WHERE project = ?1 AND mode = ?2 AND owner = ?3",
                params![p, m, o],
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
        self.sync_alerts_since(now, now_ms, &[])
    }

    /// As `sync_alerts`, but an alert raised here starts at the time given for its key in
    /// `since` (one the daemon already held in memory while the store refused writes), so what
    /// was already announced under that time stays the same alert.
    pub fn sync_alerts_since(
        &self,
        now: &[(String, String, String)],
        now_ms: u64,
        since: &[(String, u64)],
    ) -> Result<(Vec<String>, Vec<String>), StoreError> {
        let now = now.to_vec();
        let since = since.to_vec();
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
                        params![
                            key,
                            kind,
                            text,
                            since
                                .iter()
                                .find(|(k, _)| k == key)
                                .map_or(now_ms, |(_, t)| *t) as i64
                        ],
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
                "SELECT key, kind, text, since, cleared_at, acked_at, reminded_at FROM alerts WHERE (?1 = 0 OR cleared_at IS NULL) ORDER BY since, key",
            )?;
            let rows = st.query_map(params![i64::from(active_only)], |r| {
                Ok(AlertRow {
                    key: r.get(0)?,
                    kind: r.get(1)?,
                    text: r.get(2)?,
                    since: r.get::<_, i64>(3)? as u64,
                    cleared_at: r.get::<_, Option<i64>>(4)?.map(|t| t as u64),
                    acked_at: r.get::<_, Option<i64>>(5)?.map(|t| t as u64),
                    reminded_at: r.get::<_, Option<i64>>(6)?.map(|t| t as u64),
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

    /// Remember how a test command ended on this exact content tree: green, or red with
    /// the tests that failed. A later result for the same tree and command replaces it.
    pub fn put_command_run(
        &self,
        tree: &str,
        cmd: &str,
        green: bool,
        source: &str,
        names: &[String],
        now_ms: u64,
    ) -> Result<(), StoreError> {
        let (t, k, s, n) = (
            tree.to_string(),
            cmd.to_string(),
            source.to_string(),
            names.join("\n"),
        );
        self.write(move |c| {
            c.execute(
                "INSERT OR REPLACE INTO command_runs(tree, cmd, green, source, names, at) VALUES (?1,?2,?3,?4,?5,?6)",
                params![t, k, green as i64, s, n, now_ms as i64],
            )
            .map(|_| ())
        })
    }

    /// `(green, source, failing tests)` of the last recorded run of this command on this tree.
    pub fn command_run(
        &self,
        tree: &str,
        cmd: &str,
    ) -> Result<Option<(bool, String, Vec<String>)>, StoreError> {
        let (t, k) = (tree.to_string(), cmd.to_string());
        self.read(move |c| {
            c.query_row(
                "SELECT green, source, names FROM command_runs WHERE tree = ?1 AND cmd = ?2",
                params![t, k],
                |r| {
                    let names: String = r.get(2)?;
                    Ok((
                        r.get::<_, i64>(0)? != 0,
                        r.get::<_, String>(1)?,
                        names.lines().map(str::to_string).collect(),
                    ))
                },
            )
            .optional()
        })
    }

    /// A suite went green for this base: its test count is the next run's expected total.
    pub fn put_suite_green(
        &self,
        project: &str,
        suite: &str,
        base: &str,
        executed: usize,
        now_ms: u64,
    ) -> Result<(), StoreError> {
        let (p, s, b) = (project.to_string(), suite.to_string(), base.to_string());
        self.write(move |c| {
            c.execute(
                "INSERT OR REPLACE INTO suite_green(project, suite, base, executed, at_ms) VALUES (?1,?2,?3,?4,?5)",
                params![p, s, b, executed as i64, now_ms as i64],
            )
            .map(|_| ())
        })
    }

    /// The test count of the last green run of this suite: on this base if it has one, else
    /// the most recent on any base (the base moves with every landing, a suite's size hardly does).
    pub fn suite_green(
        &self,
        project: &str,
        suite: &str,
        base: &str,
    ) -> Result<Option<usize>, StoreError> {
        let (p, s, b) = (project.to_string(), suite.to_string(), base.to_string());
        self.read(move |c| {
            c.query_row(
                "SELECT executed FROM suite_green WHERE project = ?1 AND suite = ?2
                 ORDER BY (base = ?3) DESC, at_ms DESC LIMIT 1",
                params![p, s, b],
                |r| Ok(r.get::<_, i64>(0)? as usize),
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

    /// What a suite did at a base commit, with the facts a base-red comparison needs. Written
    /// once per (base, suite) and read back by [`Store::baseline_profile`].
    pub fn put_baseline_profile(
        &self,
        base: &str,
        suite: &str,
        p: &BaseProfile,
        now_ms: u64,
    ) -> Result<(), StoreError> {
        let (b, s, p) = (base.to_string(), suite.to_string(), p.clone());
        self.write(move |c| {
            c.execute(
                "INSERT OR REPLACE INTO baselines(base, suite, failing, names, executed, at,
                     build_error, vacuous, red, first_error, complete)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,1)",
                params![
                    b,
                    s,
                    p.failing.join("\n"),
                    p.names.join("\n"),
                    p.executed as i64,
                    now_ms as i64,
                    p.build_error.clone().unwrap_or_default(),
                    p.vacuous,
                    p.red,
                    p.first_error
                ],
            )
            .map(|_| ())
        })
    }

    /// The profile of a suite at a base commit, if one was recorded with every field.
    pub fn baseline_profile(
        &self,
        base: &str,
        suite: &str,
    ) -> Result<Option<BaseProfile>, StoreError> {
        let (b, s) = (base.to_string(), suite.to_string());
        self.read(move |c| {
            c.query_row(
                "SELECT failing, names, executed, build_error, vacuous, red, first_error
                 FROM baselines WHERE base = ?1 AND suite = ?2 AND complete = 1",
                params![b, s],
                |r| {
                    let build: String = r.get(3)?;
                    Ok(BaseProfile {
                        failing: r.get::<_, String>(0)?.lines().map(String::from).collect(),
                        names: r.get::<_, String>(1)?.lines().map(String::from).collect(),
                        executed: r.get::<_, i64>(2)? as usize,
                        build_error: (!build.is_empty()).then_some(build),
                        vacuous: r.get(4)?,
                        red: r.get(5)?,
                        first_error: r.get(6)?,
                    })
                },
            )
            .optional()
        })
    }

    /// A task waits in its test step because the base is red the same way (one row per task).
    pub fn put_base_red(&self, w: &BaseRedWait) -> Result<(), StoreError> {
        let w = w.clone();
        self.write(move |c| {
            c.execute(
                "INSERT OR REPLACE INTO base_red(task, suite, kind, first_error, base, text, at_ms)
                 VALUES (?1,?2,?3,?4,?5,?6,?7)",
                params![
                    w.task as i64,
                    w.suite,
                    w.kind,
                    w.first_error,
                    w.base,
                    w.text,
                    w.at_ms as i64
                ],
            )
            .map(|_| ())
        })
    }

    pub fn base_red(&self, task: u64) -> Result<Option<BaseRedWait>, StoreError> {
        self.read(move |c| {
            c.query_row(
                "SELECT suite, kind, first_error, base, text, at_ms FROM base_red WHERE task = ?1",
                params![task as i64],
                |r| {
                    Ok(BaseRedWait {
                        task,
                        suite: r.get(0)?,
                        kind: r.get(1)?,
                        first_error: r.get(2)?,
                        base: r.get(3)?,
                        text: r.get(4)?,
                        at_ms: r.get::<_, i64>(5)? as u64,
                    })
                },
            )
            .optional()
        })
    }

    pub fn clear_base_red(&self, task: u64) -> Result<(), StoreError> {
        self.write(move |c| {
            c.execute("DELETE FROM base_red WHERE task = ?1", params![task as i64])
                .map(|_| ())
        })
    }

    /// The daemon retried `task` by itself (s247).
    pub fn record_auto_retry(&self, task: u64, at_ms: u64, reason: &str) -> Result<(), StoreError> {
        let r = reason.to_string();
        self.write(move |c| {
            c.execute(
                "INSERT INTO auto_retries(task, at_ms, reason) VALUES (?1,?2,?3)",
                params![task as i64, at_ms as i64, r],
            )
            .map(|_| ())
        })
    }

    /// The times the daemon retried `task` by itself since `since_ms`, oldest first.
    pub fn auto_retries_since(&self, task: u64, since_ms: u64) -> Result<Vec<u64>, StoreError> {
        self.read(move |c| {
            let mut st = c.prepare(
                "SELECT at_ms FROM auto_retries WHERE task = ?1 AND at_ms >= ?2 ORDER BY at_ms",
            )?;
            let rows = st.query_map(params![task as i64, since_ms as i64], |r| {
                Ok(r.get::<_, i64>(0)? as u64)
            })?;
            rows.collect()
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

    /// Note that `test` failed on every run of `task`'s test step. Once per task and test: a
    /// retry of the same task adds nothing.
    pub fn record_persistent_red(
        &self,
        suite: &str,
        test: &str,
        task: u64,
        now_ms: u64,
    ) -> Result<(), StoreError> {
        let (s, t) = (suite.to_string(), test.to_string());
        self.write(move |c| {
            c.execute(
                "INSERT OR IGNORE INTO persistent_red(suite, test, task, at) VALUES (?1,?2,?3,?4)",
                params![s, t, task as i64, now_ms as i64],
            )
            .map(|_| ())
        })
    }

    /// The different tasks in which `test` failed on every run since `since_ms`.
    pub fn persistent_red_tasks(
        &self,
        suite: &str,
        test: &str,
        since_ms: u64,
    ) -> Result<Vec<u64>, StoreError> {
        let (s, t) = (suite.to_string(), test.to_string());
        self.read(move |c| {
            let mut st = c.prepare(
                "SELECT DISTINCT task FROM persistent_red WHERE suite = ?1 AND test = ?2 AND at >= ?3 ORDER BY task",
            )?;
            let rows = st.query_map(params![s, t, since_ms as i64], |r| {
                Ok(r.get::<_, i64>(0)? as u64)
            })?;
            rows.collect()
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

    /// When the task first joined the queue for this landing run: `now_ms` is recorded the
    /// first time and the recorded time is returned every time after.
    pub fn first_landing_submit(
        &self,
        task: u64,
        run: u64,
        now_ms: u64,
    ) -> Result<u64, StoreError> {
        self.write(move |c| {
            c.execute(
                "INSERT OR IGNORE INTO landing_submitted(task, run, at_ms) VALUES (?1,?2,?3)",
                params![task as i64, run as i64, now_ms as i64],
            )?;
            c.query_row(
                "SELECT at_ms FROM landing_submitted WHERE task = ?1 AND run = ?2",
                params![task as i64, run as i64],
                |r| r.get::<_, i64>(0).map(|t| t as u64),
            )
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
                "INSERT OR REPLACE INTO tracker_items(project, number, task, title, body, priority, open, mirrored, labelled) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                params![r.project, r.number as i64, r.task as i64, r.title, r.body, r.priority, r.open, r.mirrored, r.labelled],
            )
            .map(|_| ())
        })
    }

    /// The dependencies the tracker last reported: issue number to blocker issue numbers.
    pub fn tracker_deps(
        &self,
        project: &str,
    ) -> Result<std::collections::BTreeMap<u64, std::collections::BTreeSet<u64>>, StoreError> {
        let p = project.to_string();
        self.read(move |c| {
            let mut st =
                c.prepare("SELECT number, blocker_number FROM tracker_deps WHERE project = ?1")?;
            let rows = st.query_map(params![p], |r| {
                Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as u64))
            })?;
            let mut m: std::collections::BTreeMap<u64, std::collections::BTreeSet<u64>> =
                Default::default();
            for r in rows {
                let (n, b) = r?;
                m.entry(n).or_default().insert(b);
            }
            Ok(m)
        })
    }

    /// Replace what the tracker said an issue is blocked by.
    pub fn set_tracker_deps(
        &self,
        project: &str,
        number: u64,
        blockers: &[u64],
    ) -> Result<(), StoreError> {
        let p = project.to_string();
        let bs = blockers.to_vec();
        self.write(move |c| {
            c.execute(
                "DELETE FROM tracker_deps WHERE project = ?1 AND number = ?2",
                params![p, number as i64],
            )?;
            for b in &bs {
                c.execute(
                    "INSERT OR IGNORE INTO tracker_deps(project, number, blocker_number) VALUES (?1,?2,?3)",
                    params![p, number as i64, *b as i64],
                )?;
            }
            Ok(())
        })
    }

    /// Owner overrides of imported tasks' blockers: `(issue number, blocker task, kind)`.
    pub fn tracker_overrides(&self, project: &str) -> Result<Vec<(u64, u64, String)>, StoreError> {
        let p = project.to_string();
        self.read(move |c| {
            let mut st = c.prepare(
                "SELECT number, blocker_task, kind FROM tracker_overrides WHERE project = ?1 ORDER BY number, blocker_task",
            )?;
            let rows = st.query_map(params![p], |r| {
                Ok((
                    r.get::<_, i64>(0)? as u64,
                    r.get::<_, i64>(1)? as u64,
                    r.get::<_, String>(2)?,
                ))
            })?;
            rows.collect()
        })
    }

    /// Record (or replace) an owner override of one blocker of an issue's task.
    pub fn put_tracker_override(
        &self,
        project: &str,
        number: u64,
        blocker_task: u64,
        kind: &str,
        at: u64,
    ) -> Result<(), StoreError> {
        let (p, k) = (project.to_string(), kind.to_string());
        self.write(move |c| {
            c.execute(
                "INSERT OR REPLACE INTO tracker_overrides(project, number, blocker_task, kind, at) VALUES (?1,?2,?3,?4,?5)",
                params![p, number as i64, blocker_task as i64, k, at as i64],
            )
            .map(|_| ())
        })
    }

    /// Drop the overrides of an issue: of one kind, or of the one blocker task when given.
    pub fn drop_tracker_overrides(
        &self,
        project: &str,
        number: u64,
        kind: Option<&str>,
        blocker_task: Option<u64>,
    ) -> Result<(), StoreError> {
        let p = project.to_string();
        let k = kind.map(String::from);
        self.write(move |c| {
            c.execute(
                "DELETE FROM tracker_overrides WHERE project = ?1 AND number = ?2
                 AND (?3 IS NULL OR kind = ?3) AND (?4 IS NULL OR blocker_task = ?4)",
                params![p, number as i64, k, blocker_task.map(|b| b as i64)],
            )
            .map(|_| ())
        })
    }

    /// What the tracker last said each issue is blocked by (every blocker it named, imported
    /// or not), and so which issues have been looked at. `None` for an issue not yet seen.
    pub fn tracker_relationship(
        &self,
        project: &str,
        number: u64,
    ) -> Result<Option<std::collections::BTreeSet<u64>>, StoreError> {
        let p = project.to_string();
        self.read(move |c| {
            let seen: i64 = c.query_row(
                "SELECT COUNT(*) FROM tracker_relationship_seen WHERE project = ?1 AND number = ?2",
                params![p, number as i64],
                |r| r.get(0),
            )?;
            if seen == 0 {
                return Ok(None);
            }
            let mut st = c.prepare(
                "SELECT blocker_number FROM tracker_relationship WHERE project = ?1 AND number = ?2",
            )?;
            let rows = st.query_map(params![p, number as i64], |r| Ok(r.get::<_, i64>(0)? as u64))?;
            let mut s = std::collections::BTreeSet::new();
            for r in rows {
                s.insert(r?);
            }
            Ok(Some(s))
        })
    }

    pub fn set_tracker_relationship(
        &self,
        project: &str,
        number: u64,
        blockers: &[u64],
    ) -> Result<(), StoreError> {
        let p = project.to_string();
        let bs = blockers.to_vec();
        self.write(move |c| {
            c.execute(
                "DELETE FROM tracker_relationship WHERE project = ?1 AND number = ?2",
                params![p, number as i64],
            )?;
            for b in &bs {
                c.execute(
                    "INSERT OR IGNORE INTO tracker_relationship(project, number, blocker_number) VALUES (?1,?2,?3)",
                    params![p, number as i64, *b as i64],
                )?;
            }
            c.execute(
                "INSERT OR IGNORE INTO tracker_relationship_seen(project, number) VALUES (?1,?2)",
                params![p, number as i64],
            )?;
            Ok(())
        })
    }

    pub fn tracker_items(&self, project: &str) -> Result<Vec<TrackerItemRow>, StoreError> {
        let p = project.to_string();
        self.read(move |c| {
            let mut st = c.prepare(
                "SELECT project, number, task, title, body, priority, open, mirrored, labelled FROM tracker_items WHERE project = ?1 ORDER BY number",
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
                    labelled: r.get::<_, Option<i64>>(8)?.map(|v| v != 0),
                })
            })?;
            rows.collect()
        })
    }

    /// Record that a task's change needs approval. A hold for the same diff keeps its
    /// approval; a different diff starts over, because an approval is for one diff.
    pub fn put_hold(
        &self,
        task: u64,
        diff_hash: &str,
        reasons: &str,
        now_ms: u64,
    ) -> Result<HoldRow, StoreError> {
        let (h, r) = (diff_hash.to_string(), reasons.to_string());
        self.write(move |c| {
            let same: bool = c
                .query_row("SELECT diff_hash = ?2 FROM self_holds WHERE task = ?1", params![task as i64, h], |x| x.get(0))
                .optional()?
                .unwrap_or(false);
            if !same {
                c.execute(
                    "INSERT OR REPLACE INTO self_holds(task, diff_hash, reasons, at) VALUES (?1,?2,?3,?4)",
                    params![task as i64, h, r, now_ms as i64],
                )?;
            }
            Ok(())
        })?;
        Ok(self.hold(task)?.expect("just written"))
    }

    pub fn hold(&self, task: u64) -> Result<Option<HoldRow>, StoreError> {
        self.read(move |c| {
            c.query_row(
                "SELECT task, diff_hash, reasons, at, approved_by, approved_at FROM self_holds WHERE task = ?1",
                params![task as i64],
                |r| {
                    Ok(HoldRow {
                        task: r.get::<_, i64>(0)? as u64,
                        diff_hash: r.get(1)?,
                        reasons: r.get(2)?,
                        at: r.get::<_, i64>(3)? as u64,
                        approved_by: r.get(4)?,
                        approved_at: r.get::<_, Option<i64>>(5)?.map(|x| x as u64),
                    })
                },
            )
            .optional()
        })
    }

    pub fn holds(&self) -> Result<Vec<HoldRow>, StoreError> {
        self.read(move |c| {
            let mut st = c.prepare("SELECT task, diff_hash, reasons, at, approved_by, approved_at FROM self_holds ORDER BY task")?;
            let rows = st.query_map([], |r| {
                Ok(HoldRow {
                    task: r.get::<_, i64>(0)? as u64,
                    diff_hash: r.get(1)?,
                    reasons: r.get(2)?,
                    at: r.get::<_, i64>(3)? as u64,
                    approved_by: r.get(4)?,
                    approved_at: r.get::<_, Option<i64>>(5)?.map(|x| x as u64),
                })
            })?;
            rows.collect()
        })
    }

    /// Approve the hold, if there is an unapproved one. True if it was approved now.
    pub fn approve_hold(&self, task: u64, who: &str, now_ms: u64) -> Result<bool, StoreError> {
        let w = who.to_string();
        self.write(move |c| {
            c.execute(
                "UPDATE self_holds SET approved_by = ?2, approved_at = ?3 WHERE task = ?1 AND approved_by IS NULL",
                params![task as i64, w, now_ms as i64],
            )
            .map(|n| n > 0)
        })
    }

    // ---- proposals ----

    pub fn put_proposal(&self, p: &ProposalRow) -> Result<(), StoreError> {
        let p = p.clone();
        self.write(move |c| {
            c.execute(
                "INSERT OR REPLACE INTO proposals(task, project, source, severity, route, filed_by, title, body, evidence, key, status, reject_reason, publish_state, access, branch, pr, pr_state, at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18)",
                params![
                    p.task as i64, p.project, p.source, p.severity, p.route,
                    p.filed_by.map(|x| x as i64), p.title, p.body, p.evidence, p.key, p.status,
                    p.reject_reason, p.publish_state, p.access, p.branch,
                    p.pr.map(|x| x as i64), p.pr_state, p.at as i64
                ],
            )
            .map(|_| ())
        })
    }

    pub fn proposal(&self, task: u64) -> Result<Option<ProposalRow>, StoreError> {
        Ok(self
            .proposals_where("WHERE task = ?1", task as i64)?
            .into_iter()
            .next())
    }

    /// Every proposal, oldest first.
    pub fn proposals(&self) -> Result<Vec<ProposalRow>, StoreError> {
        self.proposals_where("WHERE ?1 = ?1", 0)
    }

    /// The proposals filed in `project`.
    pub fn proposals_in(&self, project: &str) -> Result<Vec<ProposalRow>, StoreError> {
        let all = self.proposals()?;
        Ok(all.into_iter().filter(|p| p.project == project).collect())
    }

    fn proposals_where(&self, cond: &str, arg: i64) -> Result<Vec<ProposalRow>, StoreError> {
        let sql = format!(
            "SELECT task, project, source, severity, route, filed_by, title, body, evidence, key, status, reject_reason, publish_state, access, branch, pr, pr_state, at FROM proposals {cond} ORDER BY task"
        );
        self.read(move |c| {
            let mut st = c.prepare(&sql)?;
            let rows = st.query_map(params![arg], |r| {
                Ok(ProposalRow {
                    task: r.get::<_, i64>(0)? as u64,
                    project: r.get(1)?,
                    source: r.get(2)?,
                    severity: r.get(3)?,
                    route: r.get(4)?,
                    filed_by: r.get::<_, Option<i64>>(5)?.map(|x| x as u64),
                    title: r.get(6)?,
                    body: r.get(7)?,
                    evidence: r.get(8)?,
                    key: r.get(9)?,
                    status: r.get(10)?,
                    reject_reason: r.get(11)?,
                    publish_state: r.get(12)?,
                    access: r.get(13)?,
                    branch: r.get(14)?,
                    pr: r.get::<_, Option<i64>>(15)?.map(|x| x as u64),
                    pr_state: r.get(16)?,
                    at: r.get::<_, i64>(17)? as u64,
                })
            })?;
            rows.collect()
        })
    }

    /// How many proposals `task` has filed.
    pub fn proposals_by_task(&self, task: u64) -> Result<u32, StoreError> {
        self.read(move |c| {
            c.query_row(
                "SELECT COUNT(*) FROM proposals WHERE filed_by = ?1",
                params![task as i64],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n as u32)
        })
    }

    /// How many agent and outside proposals `project` has had since `since_ms`.
    pub fn proposals_since(&self, project: &str, since_ms: u64) -> Result<u32, StoreError> {
        let p = project.to_string();
        self.read(move |c| {
            c.query_row(
                "SELECT COUNT(*) FROM proposals WHERE project = ?1 AND at >= ?2 AND source != 'owner'",
                params![p, since_ms as i64],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n as u32)
        })
    }

    /// Change what the owner may edit, or where a proposal stands. `f` sees the row.
    pub fn update_proposal(
        &self,
        task: u64,
        f: impl FnOnce(&mut ProposalRow),
    ) -> Result<bool, StoreError> {
        match self.proposal(task)? {
            None => Ok(false),
            Some(mut p) => {
                f(&mut p);
                self.put_proposal(&p)?;
                Ok(true)
            }
        }
    }

    pub fn clear_hold(&self, task: u64) -> Result<(), StoreError> {
        self.write(move |c| {
            c.execute(
                "DELETE FROM self_holds WHERE task = ?1",
                params![task as i64],
            )
            .map(|_| ())
        })
    }

    /// The task's number of `kind`, reserving one if it has none. The counter is the
    /// highest of `floor` (what the base branch already uses) and every number reserved
    /// for any task, plus one: it only goes up and a released number is never handed out
    /// again. Taken in one writer transaction, so two tasks never get the same number.
    /// `Ok(None)`: the next number would be above `ceiling`.
    pub fn reserve_number(
        &self,
        task: u64,
        kind: &str,
        floor: u32,
        ceiling: u32,
        now_ms: u64,
    ) -> Result<Option<ReservationRow>, StoreError> {
        let k = kind.to_string();
        let got = self.write(move |c| {
            let tx = c.unchecked_transaction()?;
            let have: Option<i64> = tx
                .query_row(
                    "SELECT number FROM number_reservations WHERE task = ?1 AND kind = ?2",
                    params![task as i64, k],
                    |r| r.get(0),
                )
                .optional()?;
            if have.is_some() {
                // A task that comes back (retried, restarted) keeps what it had.
                tx.execute(
                    "UPDATE number_reservations SET released = 0 WHERE task = ?1 AND kind = ?2",
                    params![task as i64, k],
                )?;
                tx.commit()?;
                return Ok(true);
            }
            let top: i64 = tx.query_row(
                "SELECT COALESCE(MAX(number), 0) FROM number_reservations WHERE kind = ?1",
                params![k],
                |r| r.get(0),
            )?;
            let n = top.max(floor as i64) + 1;
            if n > ceiling as i64 {
                return Ok(false);
            }
            tx.execute(
                "INSERT INTO number_reservations(task, kind, number, released, at) VALUES (?1,?2,?3,0,?4)",
                params![task as i64, k, n, now_ms as i64],
            )?;
            tx.commit()?;
            Ok(true)
        })?;
        if !got {
            return Ok(None);
        }
        Ok(self
            .reservations(task)?
            .into_iter()
            .find(|r| r.kind == kind))
    }

    /// The numbers reserved for a task.
    pub fn reservations(&self, task: u64) -> Result<Vec<ReservationRow>, StoreError> {
        Ok(self
            .all_reservations()?
            .into_iter()
            .filter(|r| r.task == task)
            .collect())
    }

    /// Every reservation, released ones included.
    pub fn all_reservations(&self) -> Result<Vec<ReservationRow>, StoreError> {
        self.read(|c| {
            let mut st = c.prepare(
                "SELECT task, kind, number, released, at FROM number_reservations ORDER BY task, kind",
            )?;
            let rows = st.query_map([], |r| {
                Ok(ReservationRow {
                    task: r.get::<_, i64>(0)? as u64,
                    kind: r.get(1)?,
                    number: r.get::<_, i64>(2)? as u32,
                    released: r.get::<_, i64>(3)? != 0,
                    at: r.get::<_, i64>(4)? as u64,
                })
            })?;
            rows.collect()
        })
    }

    /// Keep the latest record of a running step run (and its sub-counts).
    pub fn put_progress(
        &self,
        task: u64,
        p: &mason_core::progress::Progress,
    ) -> Result<(), StoreError> {
        let p = p.clone();
        self.write(move |c| {
            let tx = c.unchecked_transaction()?;
            tx.execute(
                "INSERT OR REPLACE INTO step_progress(run, task, step, unit, done, done_src, total, \
                 total_src, message, lines, bytes, marks, last_output_ms, started_ms, updated_at, max_gap_ms, tokens) \
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)",
                params![
                    p.run as i64,
                    task as i64,
                    p.step,
                    p.unit,
                    p.done.value as i64,
                    src_code(p.done.source),
                    p.total.value as i64,
                    src_code(p.total.source),
                    p.message,
                    p.output.lines as i64,
                    p.output.bytes as i64,
                    marks_text(&p.marks),
                    p.output.last_output_ms as i64,
                    p.started_ms as i64,
                    p.updated_at_ms as i64,
                    p.max_gap_ms as i64,
                    tokens_text(&p.tokens)
                ],
            )?;
            tx.execute(
                "DELETE FROM step_progress_subs WHERE run = ?1",
                params![p.run as i64],
            )?;
            for s in &p.subs {
                tx.execute(
                    "INSERT INTO step_progress_subs(run, name, unit, done, done_src, total, total_src) \
                     VALUES (?1,?2,?3,?4,?5,?6,?7)",
                    params![
                        p.run as i64,
                        s.name,
                        s.unit,
                        s.done.value as i64,
                        src_code(s.done.source),
                        s.total.value as i64,
                        src_code(s.total.source)
                    ],
                )?;
            }
            tx.commit()
        })
    }

    /// The stored records of a task's runs (`task` of None: every task's).
    pub fn progress_rows(
        &self,
        task: Option<u64>,
    ) -> Result<Vec<(u64, mason_core::progress::Progress)>, StoreError> {
        use mason_core::progress::{Figure, Output, Progress, SubProgress};
        self.read(|c| {
            let mut st = c.prepare(
                "SELECT task, run, step, unit, done, done_src, total, total_src, message, lines, bytes, \
                 marks, last_output_ms, started_ms, updated_at, max_gap_ms, tokens FROM step_progress ORDER BY run",
            )?;
            let mut out: Vec<(u64, Progress)> = vec![];
            let rows = st.query_map([], |r| {
                let g = |i: usize| r.get::<_, i64>(i).map(|v| v as u64);
                Ok((
                    g(0)?,
                    Progress {
                        run: g(1)?,
                        step: r.get(2)?,
                        unit: r.get(3)?,
                        done: Figure {
                            value: g(4)?,
                            source: src_of(r.get(5)?),
                        },
                        total: Figure {
                            value: g(6)?,
                            source: src_of(r.get(7)?),
                        },
                        message: r.get(8)?,
                        output: Output {
                            lines: g(9)?,
                            bytes: g(10)?,
                            last_output_ms: g(12)?,
                        },
                        marks: marks_of(&r.get::<_, String>(11)?),
                        started_ms: g(13)?,
                        updated_at_ms: g(14)?,
                        subs: vec![],
                        max_gap_ms: g(15)?,
                        tokens: tokens_of(&r.get::<_, String>(16)?),
                        ..Progress::default()
                    },
                ))
            })?;
            for row in rows {
                let (t, p) = row?;
                if task.is_none_or(|x| x == t) {
                    out.push((t, p));
                }
            }
            let mut sub = c.prepare(
                "SELECT name, unit, done, done_src, total, total_src FROM step_progress_subs \
                 WHERE run = ?1 ORDER BY name",
            )?;
            for (_, p) in out.iter_mut() {
                p.subs = sub
                    .query_map(params![p.run as i64], |r| {
                        Ok(SubProgress {
                            name: r.get(0)?,
                            unit: r.get(1)?,
                            done: Figure {
                                value: r.get::<_, i64>(2)? as u64,
                                source: src_of(r.get(3)?),
                            },
                            total: Figure {
                                value: r.get::<_, i64>(4)? as u64,
                                source: src_of(r.get(5)?),
                            },
                        })
                    })?
                    .collect::<Result<_, _>>()?;
            }
            Ok(out)
        })
    }

    /// Drop the stored record of a run (it ended).
    pub fn drop_progress(&self, run: u64) -> Result<(), StoreError> {
        self.write(move |c| {
            c.execute(
                "DELETE FROM step_progress WHERE run = ?1",
                params![run as i64],
            )?;
            c.execute(
                "DELETE FROM step_progress_subs WHERE run = ?1",
                params![run as i64],
            )?;
            Ok(())
        })
    }

    /// Drop every stored record whose run is not in `open` (at daemon start).
    pub fn drop_progress_except(&self, open: Vec<u64>) -> Result<usize, StoreError> {
        self.write(move |c| {
            let runs: Vec<i64> = {
                let mut st = c.prepare("SELECT run FROM step_progress")?;
                let rows = st.query_map([], |r| r.get(0))?;
                rows.collect::<Result<_, _>>()?
            };
            let mut n = 0;
            for run in runs {
                if !open.contains(&(run as u64)) {
                    c.execute("DELETE FROM step_progress WHERE run = ?1", params![run])?;
                    c.execute(
                        "DELETE FROM step_progress_subs WHERE run = ?1",
                        params![run],
                    )?;
                    n += 1;
                }
            }
            Ok(n)
        })
    }

    /// A step run succeeded: keep its final counts, for the typical figures.
    #[allow(clippy::too_many_arguments)]
    pub fn record_history(
        &self,
        project: &str,
        task: u64,
        run: u64,
        step: &str,
        done: u64,
        lines: u64,
        gap_ms: u64,
        at_ms: u64,
    ) -> Result<(), StoreError> {
        let (project, step) = (project.to_string(), step.to_string());
        self.write(move |c| {
            c.execute(
                "INSERT OR REPLACE INTO step_history(run, task, project, step, done, lines, gap_ms, at) \
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                params![
                    run as i64,
                    task as i64,
                    project,
                    step,
                    done as i64,
                    lines as i64,
                    gap_ms as i64,
                    at_ms as i64
                ],
            )?;
            Ok(())
        })
    }

    /// Append a launch row to a run; its `seq` is assigned here and returned.
    pub fn record_launch(&self, row: LaunchRow) -> Result<u64, StoreError> {
        self.write(move |c| {
            let seq: i64 = c.query_row(
                "SELECT COALESCE(MAX(seq), 0) + 1 FROM step_launches WHERE run = ?1",
                params![row.run as i64],
                |r| r.get(0),
            )?;
            c.execute(
                "INSERT INTO step_launches(run, seq, task, step, kind, tier, level, req_model, \
                 req_effort, model, effort, provider, model_from, effort_from, retry_n, retry_max, \
                 moved_up_from, overridden, reported_model, at_ms) \
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20)",
                params![
                    row.run as i64,
                    seq,
                    row.task as i64,
                    row.step,
                    row.kind,
                    row.tier,
                    row.level,
                    row.req_model,
                    row.req_effort,
                    row.model,
                    row.effort,
                    row.provider,
                    row.model_from,
                    row.effort_from,
                    row.retry_n as i64,
                    row.retry_max as i64,
                    row.moved_up_from,
                    row.overridden as i64,
                    row.reported_model,
                    row.at_ms as i64
                ],
            )?;
            Ok(seq as u64)
        })
    }

    /// The model the CLI reported for the run's latest session.
    pub fn set_reported_model(&self, run: u64, model: &str) -> Result<(), StoreError> {
        let model = model.to_string();
        self.write(move |c| {
            c.execute(
                "UPDATE step_launches SET reported_model = ?2 WHERE run = ?1 AND seq = \
                 (SELECT MAX(seq) FROM step_launches WHERE run = ?1)",
                params![run as i64, model],
            )?;
            Ok(())
        })
    }

    /// Record a lost step host (once per host id; a repeat replaces the earlier row).
    pub fn record_host_loss(&self, row: HostLossRow) -> Result<(), StoreError> {
        self.write(move |c| {
            c.execute(
                "INSERT OR REPLACE INTO step_host_losses(host_id, task, run, step, lost_at_ms, \
                 detail, retried) VALUES (?1,?2,?3,?4,?5,?6,?7)",
                params![
                    row.host_id,
                    row.task as i64,
                    row.run as i64,
                    row.step,
                    row.lost_at_ms as i64,
                    row.detail,
                    row.retried as i64
                ],
            )?;
            Ok(())
        })
    }

    /// Mark whether the step was re-run after this loss.
    pub fn set_host_loss_retried(&self, host_id: &str, retried: bool) -> Result<(), StoreError> {
        let host_id = host_id.to_string();
        self.write(move |c| {
            c.execute(
                "UPDATE step_host_losses SET retried = ?2 WHERE host_id = ?1",
                params![host_id, retried as i64],
            )?;
            Ok(())
        })
    }

    /// Every lost host of a task, oldest first.
    pub fn host_losses_for_task(&self, task: u64) -> Result<Vec<HostLossRow>, StoreError> {
        self.loss_query("WHERE task = ?1 ORDER BY lost_at_ms, host_id", task)
    }

    /// Every lost host at or after `since_ms`, oldest first.
    pub fn host_losses_since(&self, since_ms: u64) -> Result<Vec<HostLossRow>, StoreError> {
        self.loss_query(
            "WHERE lost_at_ms >= ?1 ORDER BY lost_at_ms, host_id",
            since_ms,
        )
    }

    fn loss_query(&self, tail: &'static str, key: u64) -> Result<Vec<HostLossRow>, StoreError> {
        self.read(move |c| {
            let mut st = c.prepare(&format!(
                "SELECT host_id, task, run, step, lost_at_ms, detail, retried \
                 FROM step_host_losses {tail}"
            ))?;
            let rows = st.query_map(params![key as i64], |r| {
                Ok(HostLossRow {
                    host_id: r.get(0)?,
                    task: r.get::<_, i64>(1)? as u64,
                    run: r.get::<_, i64>(2)? as u64,
                    step: r.get(3)?,
                    lost_at_ms: r.get::<_, i64>(4)? as u64,
                    detail: r.get(5)?,
                    retried: r.get::<_, i64>(6)? != 0,
                })
            })?;
            rows.collect()
        })
    }

    /// Every launch of one run, oldest first.
    pub fn launches(&self, run: u64) -> Result<Vec<LaunchRow>, StoreError> {
        self.launch_query("WHERE run = ?1 ORDER BY run, seq", run)
    }

    /// Every launch of a task, oldest run first.
    pub fn launches_for_task(&self, task: u64) -> Result<Vec<LaunchRow>, StoreError> {
        self.launch_query("WHERE task = ?1 ORDER BY run, seq", task)
    }

    fn launch_query(&self, tail: &'static str, key: u64) -> Result<Vec<LaunchRow>, StoreError> {
        self.read(move |c| {
            let mut st = c.prepare(&format!(
                "SELECT run, seq, task, step, kind, tier, level, req_model, req_effort, model, \
                 effort, provider, model_from, effort_from, retry_n, retry_max, moved_up_from, \
                 overridden, reported_model, at_ms FROM step_launches {tail}"
            ))?;
            let rows = st.query_map(params![key as i64], |r| {
                Ok(LaunchRow {
                    run: r.get::<_, i64>(0)? as u64,
                    seq: r.get::<_, i64>(1)? as u64,
                    task: r.get::<_, i64>(2)? as u64,
                    step: r.get(3)?,
                    kind: r.get(4)?,
                    tier: r.get(5)?,
                    level: r.get(6)?,
                    req_model: r.get(7)?,
                    req_effort: r.get(8)?,
                    model: r.get(9)?,
                    effort: r.get(10)?,
                    provider: r.get(11)?,
                    model_from: r.get(12)?,
                    effort_from: r.get(13)?,
                    retry_n: r.get::<_, i64>(14)? as u32,
                    retry_max: r.get::<_, i64>(15)? as u32,
                    moved_up_from: r.get(16)?,
                    overridden: r.get::<_, i64>(17)? != 0,
                    reported_model: r.get(18)?,
                    at_ms: r.get::<_, i64>(19)? as u64,
                })
            })?;
            rows.collect()
        })
    }

    /// The longest quiet gap of this project's successful runs of a step.
    pub fn history_gaps(&self, project: &str, step: &str) -> Result<Vec<u64>, StoreError> {
        let (project, step) = (project.to_string(), step.to_string());
        self.read(move |c| {
            let mut st = c.prepare(
                "SELECT gap_ms FROM step_history WHERE project = ?1 AND step = ?2 ORDER BY run",
            )?;
            let rows = st.query_map(params![project, step], |r| Ok(r.get::<_, i64>(0)? as u64))?;
            rows.collect()
        })
    }

    /// The `(done, lines)` of this project's successful runs of a step.
    pub fn history_samples(
        &self,
        project: &str,
        step: &str,
    ) -> Result<Vec<(u64, u64)>, StoreError> {
        let (project, step) = (project.to_string(), step.to_string());
        self.read(move |c| {
            let mut st = c.prepare(
                "SELECT done, lines FROM step_history WHERE project = ?1 AND step = ?2 ORDER BY run",
            )?;
            let rows = st.query_map(params![project, step], |r| {
                Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as u64))
            })?;
            rows.collect()
        })
    }

    /// Keep the latest usage snapshot of an account (opaque JSON).
    pub fn put_usage_latest(&self, account: &str, at: u64, json: &str) -> Result<(), StoreError> {
        let (a, j) = (account.to_string(), json.to_string());
        self.write(move |c| {
            c.execute(
                "INSERT OR REPLACE INTO usage_latest(account, at, snapshot) VALUES (?1,?2,?3)",
                params![a, at as i64, j],
            )?;
            Ok(())
        })
    }

    /// `(account, at, snapshot json)` for every account that has one.
    pub fn usage_latest(&self) -> Result<Vec<(String, u64, String)>, StoreError> {
        self.read(|c| {
            let mut st =
                c.prepare("SELECT account, at, snapshot FROM usage_latest ORDER BY account")?;
            let rows = st.query_map([], |r| {
                Ok((r.get(0)?, r.get::<_, i64>(1)? as u64, r.get(2)?))
            })?;
            rows.collect()
        })
    }

    /// Keep a window reading if it is the first, 5 minutes after the last, or
    /// its state or reset differs from the last. Rows older than 8 days go.
    /// Returns whether it was kept.
    pub fn record_reading(&self, r: UsageReading) -> Result<bool, StoreError> {
        self.write(move |c| {
            let last: Option<(i64, Option<i64>, String)> = c
                .query_row(
                    "SELECT at, resets_at, state FROM usage_readings \
                     WHERE account = ?1 AND win = ?2 ORDER BY at DESC LIMIT 1",
                    params![r.account, r.window],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()?;
            let keep = match last {
                None => true,
                Some((at, resets, state)) => {
                    r.at >= at as u64 + READING_GAP_SECS
                        || state != r.state
                        || resets != r.resets_at.map(|x| x as i64)
                }
            };
            if keep {
                c.execute(
                    "INSERT INTO usage_readings(account, win, at, utilization, resets_at, state) \
                     VALUES (?1,?2,?3,?4,?5,?6)",
                    params![
                        r.account,
                        r.window,
                        r.at as i64,
                        r.utilization,
                        r.resets_at.map(|x| x as i64),
                        r.state
                    ],
                )?;
            }
            c.execute(
                "DELETE FROM usage_readings WHERE at < ?1",
                params![r.at.saturating_sub(READING_KEEP_SECS) as i64],
            )?;
            Ok(keep)
        })
    }

    /// Readings at or after `since`, oldest first.
    pub fn readings_since(&self, since: u64) -> Result<Vec<UsageReading>, StoreError> {
        self.read(move |c| {
            let mut st = c.prepare(
                "SELECT account, win, at, utilization, resets_at, state FROM usage_readings \
                 WHERE at >= ?1 ORDER BY at",
            )?;
            let rows = st.query_map(params![since as i64], |r| {
                Ok(UsageReading {
                    account: r.get(0)?,
                    window: r.get(1)?,
                    at: r.get::<_, i64>(2)? as u64,
                    utilization: r.get(3)?,
                    resets_at: r.get::<_, Option<i64>>(4)?.map(|x| x as u64),
                    state: r.get(5)?,
                })
            })?;
            rows.collect()
        })
    }

    /// Hold an account (replacing any earlier hold on it).
    pub fn put_account_hold(&self, h: AccountHold) -> Result<(), StoreError> {
        self.write(move |c| {
            c.execute(
                "INSERT OR REPLACE INTO account_holds(account, kind, why, since_ms, until_ms, backoff_ms, probe_ms, by) \
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                params![
                    h.account,
                    h.kind,
                    h.why,
                    h.since_ms as i64,
                    h.until_ms.map(|x| x as i64),
                    h.backoff_ms as i64,
                    h.probe_ms as i64,
                    h.by
                ],
            )?;
            Ok(())
        })
    }

    /// Every held account.
    pub fn account_holds(&self) -> Result<Vec<AccountHold>, StoreError> {
        self.read(|c| {
            let mut st = c.prepare(
                "SELECT account, kind, why, since_ms, until_ms, backoff_ms, probe_ms, by \
                 FROM account_holds ORDER BY account",
            )?;
            let rows = st.query_map([], |r| {
                Ok(AccountHold {
                    account: r.get(0)?,
                    kind: r.get(1)?,
                    why: r.get(2)?,
                    since_ms: r.get::<_, i64>(3)? as u64,
                    until_ms: r.get::<_, Option<i64>>(4)?.map(|x| x as u64),
                    backoff_ms: r.get::<_, i64>(5)? as u64,
                    probe_ms: r.get::<_, i64>(6)? as u64,
                    by: r.get(7)?,
                })
            })?;
            rows.collect()
        })
    }

    /// Release an account. Returns whether it was held.
    pub fn clear_account_hold(&self, account: &str) -> Result<bool, StoreError> {
        let a = account.to_string();
        self.write(move |c| {
            Ok(c.execute("DELETE FROM account_holds WHERE account = ?1", params![a])? > 0)
        })
    }

    /// Note that a window's warning was given for this reset. True the first time.
    pub fn note_warning(
        &self,
        account: &str,
        window: &str,
        resets_at: u64,
        now_ms: u64,
    ) -> Result<bool, StoreError> {
        let (a, w) = (account.to_string(), window.to_string());
        self.write(move |c| {
            Ok(c.execute(
                "INSERT OR IGNORE INTO account_warnings(account, win, resets_at, at_ms) VALUES (?1,?2,?3,?4)",
                params![a, w, resets_at as i64, now_ms as i64],
            )? > 0)
        })
    }

    /// The warnings given so far, as `(account, window, resets_at)`.
    pub fn warnings(&self) -> Result<Vec<(String, String, u64)>, StoreError> {
        self.read(|c| {
            let mut st = c.prepare("SELECT account, win, resets_at FROM account_warnings")?;
            let rows = st.query_map([], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? as u64))
            })?;
            rows.collect()
        })
    }

    /// Store one step attempt's reported figures (one row per attempt key).
    pub fn record_usage_run(&self, u: UsageRun) -> Result<(), StoreError> {
        self.write(move |c| {
            let n = |x: Option<u64>| x.map(|v| v as i64);
            c.execute(
                "INSERT OR REPLACE INTO usage_runs(attempt, task, project, step, account, model, \
                 input, output, cache_read, cache_create, cost_usd, at) \
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
                params![
                    u.attempt,
                    u.task as i64,
                    u.project,
                    u.step,
                    u.account,
                    u.model,
                    n(u.input),
                    n(u.output),
                    n(u.cache_read),
                    n(u.cache_create),
                    u.cost_usd,
                    u.at as i64
                ],
            )?;
            Ok(())
        })
    }

    /// Attempts at or after `since`, oldest first.
    pub fn usage_runs_since(&self, since: u64) -> Result<Vec<UsageRun>, StoreError> {
        self.read(move |c| {
            let mut st = c.prepare(
                "SELECT attempt, task, project, step, account, model, input, output, cache_read, \
                 cache_create, cost_usd, at FROM usage_runs WHERE at >= ?1 ORDER BY at, attempt",
            )?;
            let u = |r: &rusqlite::Row, i: usize| -> rusqlite::Result<Option<u64>> {
                Ok(r.get::<_, Option<i64>>(i)?.map(|x| x as u64))
            };
            let rows = st.query_map(params![since as i64], |r| {
                Ok(UsageRun {
                    attempt: r.get(0)?,
                    task: r.get::<_, i64>(1)? as u64,
                    project: r.get(2)?,
                    step: r.get(3)?,
                    account: r.get(4)?,
                    model: r.get(5)?,
                    input: u(r, 6)?,
                    output: u(r, 7)?,
                    cache_read: u(r, 8)?,
                    cache_create: u(r, 9)?,
                    cost_usd: r.get(10)?,
                    at: r.get::<_, i64>(11)? as u64,
                })
            })?;
            rows.collect()
        })
    }

    /// The task is cancelled: mark its numbers released. They stay taken.
    pub fn release_numbers(&self, task: u64) -> Result<usize, StoreError> {
        self.write(move |c| {
            c.execute(
                "UPDATE number_reservations SET released = 1 WHERE task = ?1 AND released = 0",
                params![task as i64],
            )
        })
    }

    /// Freeze landing because the post-land tier is red. Never replaces a freeze the CI watch holds.
    pub fn set_freeze(
        &self,
        project: &str,
        reason: &str,
        fix_task: Option<u64>,
        now_ms: u64,
    ) -> Result<(), StoreError> {
        self.set_freeze_from("post-land", project, reason, fix_task, now_ms)
    }

    /// Freeze landing because a gate job is red on the base branch. Replaces its own freeze
    /// (a later pass may know the fix task), never a post-land one.
    pub fn set_ci_freeze(
        &self,
        project: &str,
        reason: &str,
        fix_task: Option<u64>,
        now_ms: u64,
    ) -> Result<(), StoreError> {
        self.set_freeze_from("ci", project, reason, fix_task, now_ms)
    }

    fn set_freeze_from(
        &self,
        source: &'static str,
        project: &str,
        reason: &str,
        fix_task: Option<u64>,
        now_ms: u64,
    ) -> Result<(), StoreError> {
        let (p, r) = (project.to_string(), reason.to_string());
        self.write(move |c| {
            // The same source updates its row but keeps when it began.
            c.execute(
                "INSERT INTO freezes(project, since, reason, fix_task, source) VALUES (?1,?2,?3,?4,?5)
                 ON CONFLICT(project) DO UPDATE SET reason = excluded.reason, fix_task = excluded.fix_task
                 WHERE freezes.source = excluded.source",
                params![p, now_ms as i64, r, fix_task.map(|t| t as i64), source],
            )
            .map(|_| ())
        })
    }

    /// Lift the post-land freeze. A freeze the CI watch holds stays.
    pub fn clear_freeze(&self, project: &str) -> Result<bool, StoreError> {
        self.clear_freeze_from("post-land", project)
    }

    /// Lift the CI watch's freeze. A post-land freeze stays.
    pub fn clear_ci_freeze(&self, project: &str) -> Result<bool, StoreError> {
        self.clear_freeze_from("ci", project)
    }

    fn clear_freeze_from(&self, source: &'static str, project: &str) -> Result<bool, StoreError> {
        let p = project.to_string();
        self.write(move |c| {
            c.execute(
                "DELETE FROM freezes WHERE project = ?1 AND source = ?2",
                params![p, source],
            )
            .map(|n| n > 0)
        })
    }

    pub fn freezes(&self) -> Result<Vec<FreezeRow>, StoreError> {
        self.read(move |c| {
            let mut st = c.prepare(
                "SELECT project, since, reason, fix_task, source FROM freezes ORDER BY project",
            )?;
            let rows = st.query_map([], |r| {
                Ok(FreezeRow {
                    project: r.get(0)?,
                    since: r.get::<_, i64>(1)? as u64,
                    reason: r.get(2)?,
                    fix_task: r.get::<_, Option<i64>>(3)?.map(|t| t as u64),
                    source: r.get(4)?,
                })
            })?;
            rows.collect()
        })
    }

    /// What the base-branch CI watch last saw for a project (text; the daemon owns its form).
    pub fn ci_watch_state(&self, project: &str) -> Result<Option<String>, StoreError> {
        let p = project.to_string();
        self.read(move |c| {
            c.query_row(
                "SELECT state FROM ci_watch WHERE project = ?1",
                params![p],
                |r| r.get(0),
            )
            .optional()
        })
    }

    pub fn put_ci_watch_state(
        &self,
        project: &str,
        state: &str,
        now_ms: u64,
    ) -> Result<(), StoreError> {
        let (p, s) = (project.to_string(), state.to_string());
        self.write(move |c| {
            c.execute(
                "INSERT OR REPLACE INTO ci_watch(project, state, at) VALUES (?1,?2,?3)",
                params![p, s, now_ms as i64],
            )
            .map(|_| ())
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
        let conn = self.reader.lock().unwrap_or_else(|e| e.into_inner());
        let mut st = conn.prepare("SELECT at, kind, detail FROM bugs ORDER BY id")?;
        let rows = st.query_map([], |r| {
            Ok((r.get::<_, i64>(0)? as u64, r.get(1)?, r.get(2)?))
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    /// Bugs filed at or after `since_ms`: `(at_ms, kind, detail)`, oldest first.
    pub fn bugs_since(&self, since_ms: u64) -> Result<Vec<(u64, String, String)>, StoreError> {
        let conn = self.reader.lock().unwrap_or_else(|e| e.into_inner());
        let mut st =
            conn.prepare("SELECT at, kind, detail FROM bugs WHERE at >= ?1 ORDER BY id")?;
        let rows = st.query_map(params![since_ms as i64], |r| {
            Ok((r.get::<_, i64>(0)? as u64, r.get(1)?, r.get(2)?))
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    /// The projection tables as sorted text lines, for tests and diagnostics.
    pub fn projection_dump(&self) -> Result<Vec<String>, StoreError> {
        let conn = self.reader.lock().unwrap_or_else(|e| e.into_inner());
        dump_db(&conn)
    }

    /// Check that the log is contiguous and replays cleanly, and that the
    /// projections match that replay exactly (nothing half-applied).
    /// A consistent copy of the whole database in a new file (`VACUUM INTO`):
    /// the backup taken before a new build takes over, and the copy a new
    /// build proves its migrations on.
    pub fn backup_to(&self, dest: &Path) -> Result<(), StoreError> {
        let d = dest.display().to_string();
        let _ = std::fs::remove_file(dest);
        self.read(move |c| c.execute("VACUUM INTO ?1", params![d]).map(|_| ()))
    }

    pub fn verify(&self) -> Result<(), StoreError> {
        // Its own connection: the replay can take seconds, and must not hold the shared
        // reader's mutex while every other read in the daemon waits.
        let conn = Connection::open_with_flags(
            &self.path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        // One read snapshot for every query below. Without it, an event appended between
        // the count and the last sequence number reads as a gap in a healthy log, and the
        // replay and the projection dump could be taken at different points.
        let _snapshot = conn.unchecked_transaction()?;
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

fn src_code(s: mason_core::progress::Source) -> i64 {
    use mason_core::progress::Source::*;
    match s {
        None => 0,
        Reported => 1,
        Counted => 2,
        History => 3,
        Expected => 4,
        Minimum => 5,
    }
}

fn src_of(n: i64) -> mason_core::progress::Source {
    use mason_core::progress::Source::*;
    match n {
        1 => Reported,
        2 => Counted,
        3 => History,
        4 => Expected,
        5 => Minimum,
        _ => None,
    }
}

fn marks_text(m: &std::collections::BTreeMap<String, u64>) -> String {
    m.iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(";")
}

fn tokens_text(m: &std::collections::BTreeMap<String, mason_core::progress::Tokens>) -> String {
    let f = |v: Option<u64>| v.map(|n| n.to_string()).unwrap_or_default();
    m.iter()
        .map(|(k, t)| {
            format!(
                "{k}={},{},{},{},{}",
                f(t.input),
                f(t.output),
                f(t.cache_read),
                f(t.cache_write),
                f(t.cost_nano_usd)
            )
        })
        .collect::<Vec<_>>()
        .join(";")
}

fn tokens_of(t: &str) -> std::collections::BTreeMap<String, mason_core::progress::Tokens> {
    t.split(';')
        .filter_map(|p| {
            let (k, v) = p.rsplit_once('=')?;
            let f: Vec<Option<u64>> = v.split(',').map(|x| x.parse().ok()).collect();
            if f.len() != 5 {
                return None;
            }
            Some((
                k.to_string(),
                mason_core::progress::Tokens {
                    input: f[0],
                    output: f[1],
                    cache_read: f[2],
                    cache_write: f[3],
                    cost_nano_usd: f[4],
                },
            ))
        })
        .collect()
}

fn marks_of(t: &str) -> std::collections::BTreeMap<String, u64> {
    t.split(';')
        .filter_map(|p| {
            let (k, v) = p.rsplit_once('=')?;
            Some((k.to_string(), v.parse().ok()?))
        })
        .collect()
}

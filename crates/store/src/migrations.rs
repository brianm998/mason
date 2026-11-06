// SPDX-License-Identifier: GPL-3.0-or-later

use rusqlite::Connection;

/// Each step takes the schema from version `i` to `i + 1`.
const STEPS: &[&str] = &[
    // 1: the append-only log.
    "CREATE TABLE events (
         seq     INTEGER PRIMARY KEY AUTOINCREMENT,
         at      INTEGER NOT NULL,
         kind    TEXT NOT NULL,
         payload TEXT NOT NULL
     );
     CREATE TRIGGER events_no_update BEFORE UPDATE ON events
         BEGIN SELECT RAISE(ABORT, 'events are append-only'); END;
     CREATE TRIGGER events_no_delete BEFORE DELETE ON events
         BEGIN SELECT RAISE(ABORT, 'events are append-only'); END;
     CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
    // 2: projections, rebuilt from the log.
    "CREATE TABLE projects (id INTEGER PRIMARY KEY, name TEXT NOT NULL);
     CREATE TABLE tasks (
         id              INTEGER PRIMARY KEY,
         project         INTEGER NOT NULL,
         title           TEXT NOT NULL,
         state           TEXT NOT NULL,
         activity_kind   TEXT NOT NULL,
         activity_detail TEXT NOT NULL,
         priority        TEXT NOT NULL,
         enqueued_at     INTEGER NOT NULL,
         failures        INTEGER NOT NULL,
         decision        INTEGER
     );
     CREATE INDEX tasks_by_state ON tasks(state);
     CREATE TABLE decisions (
         id       INTEGER PRIMARY KEY,
         task     INTEGER NOT NULL,
         question TEXT NOT NULL,
         answer   TEXT
     );
     INSERT OR REPLACE INTO meta(key, value) VALUES ('projected_seq', '0');",
    // 3: bugs the reconcile sweep filed, when it found something the event
    // path missed. Not part of the event log: they describe the pipeline, not
    // a task.
    "CREATE TABLE bugs (
         id     INTEGER PRIMARY KEY AUTOINCREMENT,
         at     INTEGER NOT NULL,
         kind   TEXT NOT NULL,
         detail TEXT NOT NULL
     );",
    // 4: definitions a task can be pinned to, configuration overrides (runtime,
    // each with an owner and an expiry, and per task), and the step, route and
    // definition on the task projection. The projection is rebuilt from the log
    // on the next open so the new columns fill in.
    "CREATE TABLE definitions (
         hash TEXT PRIMARY KEY,
         text TEXT NOT NULL,
         at   INTEGER NOT NULL
     );
     CREATE TABLE overrides (
         id         INTEGER PRIMARY KEY AUTOINCREMENT,
         project    TEXT NOT NULL,
         key        TEXT NOT NULL,
         value      TEXT NOT NULL,
         owner      TEXT NOT NULL,
         created_at INTEGER NOT NULL,
         expires_at INTEGER NOT NULL
     );
     CREATE TABLE task_overrides (
         task       INTEGER NOT NULL,
         key        TEXT NOT NULL,
         value      TEXT NOT NULL,
         owner      TEXT NOT NULL,
         created_at INTEGER NOT NULL,
         PRIMARY KEY (task, key)
     );
     ALTER TABLE tasks ADD COLUMN step  TEXT NOT NULL DEFAULT '';
     ALTER TABLE tasks ADD COLUMN route TEXT NOT NULL DEFAULT '';
     ALTER TABLE tasks ADD COLUMN def   TEXT NOT NULL DEFAULT '';
     UPDATE meta SET value = '0' WHERE key = 'projected_seq';",
    // 5: the testing system's memory. Evidence per tree hash and suite (so
    // identical code never tests twice), baseline failures per base commit,
    // the flaky log, debts (what was skipped by force and not yet paid),
    // quarantine (always with an owner and an expiry), and each task's closing
    // verdict. Not part of the event log: they describe code, not a task's
    // progress, and evidence outlives any one task.
    "CREATE TABLE evidence (
         tree     TEXT NOT NULL,
         suite    TEXT NOT NULL,
         executed INTEGER NOT NULL,
         passed   INTEGER NOT NULL,
         at       INTEGER NOT NULL,
         task     INTEGER NOT NULL,
         PRIMARY KEY (tree, suite)
     );
     CREATE TABLE baselines (
         base     TEXT NOT NULL,
         suite    TEXT NOT NULL,
         failing  TEXT NOT NULL,
         names    TEXT NOT NULL,
         executed INTEGER NOT NULL,
         at       INTEGER NOT NULL,
         PRIMARY KEY (base, suite)
     );
     CREATE TABLE flaky (
         id    INTEGER PRIMARY KEY AUTOINCREMENT,
         at    INTEGER NOT NULL,
         suite TEXT NOT NULL,
         test  TEXT NOT NULL,
         task  INTEGER NOT NULL
     );
     CREATE TABLE debts (
         id       INTEGER PRIMARY KEY AUTOINCREMENT,
         at       INTEGER NOT NULL,
         task     INTEGER NOT NULL,
         project  TEXT NOT NULL,
         scope    TEXT NOT NULL,
         reason   TEXT NOT NULL,
         owner    TEXT NOT NULL,
         deferred INTEGER NOT NULL,
         status   TEXT NOT NULL,
         paid_at  INTEGER
     );
     CREATE TABLE quarantine (
         suite      TEXT NOT NULL,
         test       TEXT NOT NULL,
         owner      TEXT NOT NULL,
         expires_at INTEGER NOT NULL,
         reason     TEXT NOT NULL,
         at         INTEGER NOT NULL,
         PRIMARY KEY (suite, test)
     );
     CREATE TABLE closings (
         task    INTEGER PRIMARY KEY,
         verdict TEXT NOT NULL,
         note    TEXT NOT NULL,
         at      INTEGER NOT NULL
     );",
    // 6: the owner's interventions (the metric's numerator), pauses (runtime
    // overrides with an owner and an expiry, per project or global), and alerts
    // tied to live conditions (raised when a condition appears, cleared when it
    // goes). Like the testing tables, these describe the pipeline rather than a
    // task's progress, so they are not in the event log.
    "CREATE TABLE interventions (
         id      INTEGER PRIMARY KEY AUTOINCREMENT,
         at      INTEGER NOT NULL,
         kind    TEXT NOT NULL,
         task    INTEGER,
         project TEXT NOT NULL,
         who     TEXT NOT NULL,
         detail  TEXT NOT NULL,
         reason  TEXT NOT NULL
     );
     CREATE TABLE pauses (
         id         INTEGER PRIMARY KEY AUTOINCREMENT,
         project    TEXT NOT NULL,
         mode       TEXT NOT NULL,
         owner      TEXT NOT NULL,
         created_at INTEGER NOT NULL,
         expires_at INTEGER NOT NULL
     );
     CREATE TABLE alerts (
         key        TEXT PRIMARY KEY,
         kind       TEXT NOT NULL,
         text       TEXT NOT NULL,
         since      INTEGER NOT NULL,
         cleared_at INTEGER
     );",
    // 7: landing (Part 9). Batches the landing engine formed and how each
    // ended; a freeze per project (the post-land tier went red: only fixes
    // land until it is green again); and the owner's land-now and force marks,
    // which a task carries into the queue. Like debts, these describe the
    // pipeline rather than a task's progress, so they are not in the event log.
    "CREATE TABLE batches (
         id          INTEGER PRIMARY KEY AUTOINCREMENT,
         project     TEXT NOT NULL,
         at          INTEGER NOT NULL,
         finished_at INTEGER,
         phase       TEXT NOT NULL,
         tasks       TEXT NOT NULL,
         landed      TEXT NOT NULL DEFAULT '',
         parked      TEXT NOT NULL DEFAULT '',
         note        TEXT NOT NULL DEFAULT '',
         head        TEXT NOT NULL DEFAULT ''
     );
     CREATE TABLE freezes (
         project  TEXT PRIMARY KEY,
         since    INTEGER NOT NULL,
         reason   TEXT NOT NULL,
         fix_task INTEGER
     );
     CREATE TABLE landing_flags (
         task  INTEGER PRIMARY KEY,
         kind  TEXT NOT NULL,
         at    INTEGER NOT NULL,
         owner TEXT NOT NULL
     );",
];

pub const SCHEMA_VERSION: u32 = STEPS.len() as u32;

pub fn version(conn: &Connection) -> rusqlite::Result<u32> {
    conn.query_row("PRAGMA user_version", [], |r| r.get(0))
}

/// Bring the database up to `target` (at most [`SCHEMA_VERSION`]), one
/// transaction per step. Exposed so tests can build old-version databases.
pub fn migrate_to(conn: &mut Connection, target: u32) -> rusqlite::Result<()> {
    let target = target.min(SCHEMA_VERSION);
    let mut v = version(conn)?;
    while v < target {
        let tx = conn.transaction()?;
        tx.execute_batch(STEPS[v as usize])?;
        tx.pragma_update(None, "user_version", v + 1)?;
        tx.commit()?;
        v += 1;
    }
    Ok(())
}

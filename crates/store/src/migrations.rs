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

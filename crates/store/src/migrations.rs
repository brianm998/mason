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
    // 8: what a batch put on the outside (candidate refs for CI, pull
    // requests), so that whatever a crash leaves there is found and removed
    // on restart, and the tracker's items, with what the tracker last said.
    "CREATE TABLE landing_refs (
         id      INTEGER PRIMARY KEY AUTOINCREMENT,
         batch   INTEGER NOT NULL,
         project TEXT NOT NULL,
         kind    TEXT NOT NULL,
         name    TEXT NOT NULL,
         number  INTEGER,
         at      INTEGER NOT NULL,
         removed INTEGER NOT NULL DEFAULT 0
     );
     CREATE TABLE tracker_items (
         project  TEXT NOT NULL,
         number   INTEGER NOT NULL,
         task     INTEGER NOT NULL,
         title    TEXT NOT NULL,
         body     TEXT NOT NULL,
         priority TEXT NOT NULL,
         open     INTEGER NOT NULL,
         mirrored TEXT NOT NULL DEFAULT '',
         PRIMARY KEY (project, number)
     );",
    // 9: self-change holds (Part 13): a change to MASON's own repository that
    // touches a gate, guard, budget, verdict or trust rule (or weakens one) waits
    // here for the owner, and the approval is for that exact diff.
    "CREATE TABLE self_holds (
         task        INTEGER PRIMARY KEY,
         diff_hash   TEXT NOT NULL,
         reasons     TEXT NOT NULL,
         at          INTEGER NOT NULL,
         approved_by TEXT,
         approved_at INTEGER
     );",
    // 10: the dependencies the tracker last said an issue has (issue number to
    // blocker issue number), so a change there can be told from an `--after`
    // edge and applied to the task.
    "CREATE TABLE tracker_deps (
         project        TEXT NOT NULL,
         number         INTEGER NOT NULL,
         blocker_number INTEGER NOT NULL,
         PRIMARY KEY (project, number, blocker_number)
     );",
    // 11: numbers MASON hands out (decision records and exit scripts / stage notes), one
    // row per task and kind. A number is never given twice, not even after the task that
    // held it is cancelled (the row is kept, marked released).
    "CREATE TABLE number_reservations (
         task     INTEGER NOT NULL,
         kind     TEXT NOT NULL CHECK (kind IN ('decision', 'exit')),
         number   INTEGER NOT NULL,
         released INTEGER NOT NULL DEFAULT 0,
         at       INTEGER NOT NULL,
         PRIMARY KEY (task, kind),
         UNIQUE (kind, number)
     );",
    // 12: the live progress of each open step run (written every few seconds, so a restart
    // shows the same figures and counts on from there), its sub-counts, and the final counts
    // of successful runs (the history the typical figures come from).
    "CREATE TABLE step_progress (
         run            INTEGER PRIMARY KEY,
         task           INTEGER NOT NULL,
         step           TEXT NOT NULL,
         unit           TEXT NOT NULL,
         done           INTEGER NOT NULL,
         done_src       INTEGER NOT NULL,
         total          INTEGER NOT NULL,
         total_src      INTEGER NOT NULL,
         message        TEXT NOT NULL,
         lines          INTEGER NOT NULL,
         bytes          INTEGER NOT NULL,
         marks          TEXT NOT NULL,
         last_output_ms INTEGER NOT NULL,
         started_ms     INTEGER NOT NULL,
         updated_at     INTEGER NOT NULL
     );
     CREATE INDEX step_progress_task ON step_progress(task);
     CREATE TABLE step_progress_subs (
         run       INTEGER NOT NULL,
         name      TEXT NOT NULL,
         unit      TEXT NOT NULL,
         done      INTEGER NOT NULL,
         done_src  INTEGER NOT NULL,
         total     INTEGER NOT NULL,
         total_src INTEGER NOT NULL,
         PRIMARY KEY (run, name)
     );
     CREATE TABLE step_history (
         run     INTEGER PRIMARY KEY,
         task    INTEGER NOT NULL,
         project TEXT NOT NULL,
         step    TEXT NOT NULL,
         done    INTEGER NOT NULL,
         lines   INTEGER NOT NULL,
         at      INTEGER NOT NULL
     );
     CREATE INDEX step_history_step ON step_history(project, step);",
    // 13: the longest quiet gap of each successful run, for the "no output for" threshold (s49).
    "ALTER TABLE step_history ADD COLUMN gap_ms INTEGER NOT NULL DEFAULT 0;\n     ALTER TABLE step_progress ADD COLUMN max_gap_ms INTEGER NOT NULL DEFAULT 0;",
    // 14: what each step run actually launched (s63): the requested tier and level, the real
    // model and effort passed to the CLI, where each came from, and the model the CLI reported.
    // Written by the daemon beside step_progress; not rebuilt by event replay.
    "CREATE TABLE step_launches (
         run             INTEGER NOT NULL,
         seq             INTEGER NOT NULL,
         task            INTEGER NOT NULL,
         step            TEXT NOT NULL,
         kind            TEXT NOT NULL,
         tier            TEXT NOT NULL,
         level           TEXT NOT NULL,
         req_model       TEXT NOT NULL,
         req_effort      TEXT NOT NULL,
         model           TEXT NOT NULL,
         effort          TEXT NOT NULL,
         provider        TEXT NOT NULL,
         model_from      TEXT NOT NULL,
         effort_from     TEXT NOT NULL,
         retry_n         INTEGER NOT NULL,
         retry_max       INTEGER NOT NULL,
         moved_up_from   TEXT NOT NULL,
         overridden      INTEGER NOT NULL,
         reported_model  TEXT NOT NULL,
         at_ms           INTEGER NOT NULL,
         PRIMARY KEY (run, seq)
     );
     CREATE INDEX step_launches_task ON step_launches(task);",
    // 15: proposals (Part 13). A proposal is a task plus this row: what the agent saw (evidence),
    // where it was routed, the key it was deduplicated on, and what has been done with it. An
    // improvement the owner asked for (source 'owner') is the same row with a branch.
    "CREATE TABLE proposals (
         task          INTEGER PRIMARY KEY,
         project       TEXT NOT NULL,
         source        TEXT NOT NULL,
         severity      TEXT NOT NULL,
         route         TEXT NOT NULL,
         filed_by      INTEGER,
         title         TEXT NOT NULL,
         body          TEXT NOT NULL,
         evidence      TEXT NOT NULL,
         key           TEXT NOT NULL,
         status        TEXT NOT NULL DEFAULT 'open',
         reject_reason TEXT NOT NULL DEFAULT '',
         publish_state TEXT NOT NULL DEFAULT 'local',
         access        TEXT NOT NULL DEFAULT '',
         branch        TEXT NOT NULL DEFAULT '',
         pr            INTEGER,
         pr_state      TEXT NOT NULL DEFAULT '',
         at            INTEGER NOT NULL
     );
     CREATE INDEX proposals_project_at ON proposals(project, at);
     CREATE INDEX proposals_filed_by ON proposals(filed_by);",
    // 16: LLM usage (s64). The latest snapshot per account (kept across a restart), a bounded
    // history of window readings (the trend), and the tokens and cost of each step attempt as
    // the provider reported them (NULL: not reported, never 0).
    "CREATE TABLE usage_latest (
         account  TEXT PRIMARY KEY,
         at       INTEGER NOT NULL,
         snapshot TEXT NOT NULL
     );
     CREATE TABLE usage_readings (
         account     TEXT NOT NULL,
         win         TEXT NOT NULL,
         at          INTEGER NOT NULL,
         utilization REAL,
         resets_at   INTEGER,
         state       TEXT NOT NULL
     );
     CREATE INDEX usage_readings_win ON usage_readings(account, win, at);
     CREATE TABLE usage_runs (
         attempt      TEXT PRIMARY KEY,
         task         INTEGER NOT NULL,
         project      TEXT NOT NULL,
         step         TEXT NOT NULL,
         account      TEXT NOT NULL,
         model        TEXT,
         input        INTEGER,
         output       INTEGER,
         cache_read   INTEGER,
         cache_create INTEGER,
         cost_usd     REAL,
         at           INTEGER NOT NULL
     );
     CREATE INDEX usage_runs_at ON usage_runs(at);
     CREATE INDEX usage_runs_task ON usage_runs(task);",
    // 17: introspection and recurring jobs (Part 14). `step_attempts` is one row per step
    // attempt, written when the attempt ends, with the failure signature: the history the
    // detectors read. `findings` links a detector's or the analyst's proposal to the metric it
    // expects to move, and keeps the verdict. `schedules` remembers when each recurring job last
    // ran and the failures its baseline has accepted. `triage_samples` is the owner's review
    // queue for the optional triage agent.
    "CREATE TABLE step_attempts (
         id        INTEGER PRIMARY KEY AUTOINCREMENT,
         task      INTEGER NOT NULL,
         project   TEXT NOT NULL,
         step      TEXT NOT NULL,
         outcome   TEXT NOT NULL,
         reason    TEXT NOT NULL,
         signature TEXT NOT NULL,
         retry_n   INTEGER NOT NULL,
         at        INTEGER NOT NULL
     );
     CREATE INDEX step_attempts_at ON step_attempts(at);
     CREATE INDEX step_attempts_task ON step_attempts(task);
     CREATE TABLE findings (
         id         INTEGER PRIMARY KEY AUTOINCREMENT,
         rule       TEXT NOT NULL,
         subject    TEXT NOT NULL,
         project    TEXT NOT NULL,
         metric     TEXT NOT NULL,
         title      TEXT NOT NULL,
         task       INTEGER NOT NULL,
         before_num REAL NOT NULL,
         before_den REAL NOT NULL,
         before_n   INTEGER NOT NULL,
         at         INTEGER NOT NULL,
         verdict    TEXT NOT NULL DEFAULT '',
         verdict_text TEXT NOT NULL DEFAULT '',
         after_num  REAL NOT NULL DEFAULT 0,
         after_den  REAL NOT NULL DEFAULT 0,
         after_n    INTEGER NOT NULL DEFAULT 0,
         landed_at  INTEGER NOT NULL DEFAULT 0,
         closed_at  INTEGER NOT NULL DEFAULT 0
     );
     CREATE INDEX findings_task ON findings(task);
     CREATE TABLE schedules (
         name     TEXT PRIMARY KEY,
         project  TEXT NOT NULL,
         kind     TEXT NOT NULL,
         every_ms INTEGER NOT NULL,
         last_run INTEGER NOT NULL DEFAULT 0,
         last_task INTEGER,
         last_status TEXT NOT NULL DEFAULT '',
         baseline TEXT,
         runs     INTEGER NOT NULL DEFAULT 0
     );
     CREATE TABLE triage_samples (
         id       INTEGER PRIMARY KEY AUTOINCREMENT,
         task     INTEGER NOT NULL,
         question TEXT NOT NULL,
         action   TEXT NOT NULL,
         answer   TEXT NOT NULL,
         evidence TEXT NOT NULL,
         status   TEXT NOT NULL,
         verdict  TEXT NOT NULL DEFAULT '',
         at       INTEGER NOT NULL
      );",
    // 18: account holds (s67). One row per held account, so a restart during a hold keeps it;
    // and the warnings already given (one per window per reset).
    "CREATE TABLE account_holds (
         account    TEXT PRIMARY KEY,
         kind       TEXT NOT NULL,
         why        TEXT NOT NULL,
         since_ms   INTEGER NOT NULL,
         until_ms   INTEGER,
         backoff_ms INTEGER NOT NULL DEFAULT 0,
         probe_ms   INTEGER NOT NULL DEFAULT 0,
         by         TEXT NOT NULL DEFAULT 'pipeline'
     );
     CREATE TABLE account_warnings (
         account   TEXT NOT NULL,
         win       TEXT NOT NULL,
         resets_at INTEGER NOT NULL,
         at_ms     INTEGER NOT NULL,
         PRIMARY KEY (account, win, resets_at)
     );",
    // 18: notifications (Part 19). An alert can be acknowledged (which stops its noise but not
    // its visibility) and is reminded at most once. `deliveries` is every attempt to reach the
    // owner by a channel, so a notice is sent once and a reminder once. `announcements` is the
    // queue of sentences for whichever device speaks; its unique key makes an announcement
    // happen once however often the dispatcher looks.
    "ALTER TABLE alerts ADD COLUMN acked_at INTEGER;
     ALTER TABLE alerts ADD COLUMN reminded_at INTEGER;
     CREATE TABLE deliveries (
         id       INTEGER PRIMARY KEY AUTOINCREMENT,
         alert    TEXT NOT NULL,
         since    INTEGER NOT NULL,
         channel  TEXT NOT NULL,
         delivery TEXT NOT NULL,
         ok       INTEGER NOT NULL,
         detail   TEXT NOT NULL,
         at       INTEGER NOT NULL
     );
     CREATE INDEX deliveries_alert ON deliveries(alert, since);
     CREATE TABLE announcements (
         id         INTEGER PRIMARY KEY AUTOINCREMENT,
         key        TEXT NOT NULL UNIQUE,
         kind       TEXT NOT NULL,
         sentence   TEXT NOT NULL,
         sound      INTEGER NOT NULL,
         speech     INTEGER NOT NULL,
         created_at INTEGER NOT NULL,
         taken_at   INTEGER,
         taken_by   TEXT
     );",
    // 19: paired devices (s24). Only the SHA-256 of a token is kept; the token is shown once.
    "CREATE TABLE devices (
         id           INTEGER PRIMARY KEY AUTOINCREMENT,
         name         TEXT NOT NULL,
         role         TEXT NOT NULL,
         token_sha256 TEXT NOT NULL UNIQUE,
         created_at   INTEGER NOT NULL,
         last_seen    INTEGER,
         revoked_at   INTEGER
     );",
    // 20: step hosts that were lost without an exit record (s66): what was known about each,
    // and whether the step was re-run on a fresh host. Written by the daemon; not rebuilt
    // by event replay.
    "CREATE TABLE step_host_losses (
         host_id    TEXT PRIMARY KEY,
         task       INTEGER NOT NULL,
         run        INTEGER NOT NULL,
         step       TEXT NOT NULL,
         lost_at_ms INTEGER NOT NULL,
         detail     TEXT NOT NULL,
         retried    INTEGER NOT NULL
     );
     CREATE INDEX step_host_losses_task ON step_host_losses(task);
     CREATE INDEX step_host_losses_at ON step_host_losses(lost_at_ms);",
    // 21: owner overrides of an imported task's blockers (s75), and the relationship the
    // tracker last reported for each issue (to tell when it changed). Written by the daemon;
    // like tracker_deps they are not rebuilt by event replay. kind: 'release' (the owner
    // released a blocker GitHub lists) or 'add' (the owner added a blocker GitHub does not).
    "CREATE TABLE tracker_overrides (
         project       TEXT NOT NULL,
         number        INTEGER NOT NULL,
         blocker_task  INTEGER NOT NULL,
         kind          TEXT NOT NULL CHECK (kind IN ('release', 'add')),
         at            INTEGER NOT NULL,
         PRIMARY KEY (project, number, blocker_task)
     );
     CREATE TABLE tracker_relationship (
         project        TEXT NOT NULL,
         number         INTEGER NOT NULL,
         blocker_number INTEGER NOT NULL,
         PRIMARY KEY (project, number, blocker_number)
     );
     CREATE TABLE tracker_relationship_seen (
         project TEXT NOT NULL,
         number  INTEGER NOT NULL,
         PRIMARY KEY (project, number)
     );",
    // 22: the last green count of a suite for a base commit (s70), the expected
    // total of a live test count. Kept apart from `evidence` (keyed by tree).
    "CREATE TABLE suite_green (
         project  TEXT NOT NULL,
         suite    TEXT NOT NULL,
         base     TEXT NOT NULL,
         executed INTEGER NOT NULL,
         at_ms    INTEGER NOT NULL,
         PRIMARY KEY (project, suite, base)
     );",
    // 23: the tokens each running step run's LLM sessions have used (s76): one tally per
    // output stream, as `stream=in,out,cache_read,cache_write,cost;...` (an empty field is
    // a figure not reported). Additive; a row from before has none.
    "ALTER TABLE step_progress ADD COLUMN tokens TEXT NOT NULL DEFAULT '';",
    // 24: whether each imported issue carried the project's label at the last look (s136,
    // decision 0131). NULL: not looked at since. Additive.
    "ALTER TABLE tracker_items ADD COLUMN labelled INTEGER;",
    // 25: the base-branch CI watch (s154, decision 0149): what it last saw, per project, as
    // text the daemon reads and writes; and who set each freeze, so the post-land tier going
    // green does not lift a freeze that the CI watch holds, and the other way round.
    "CREATE TABLE ci_watch (
         project TEXT PRIMARY KEY,
         state   TEXT NOT NULL,
         at      INTEGER NOT NULL
     );
     ALTER TABLE freezes ADD COLUMN source TEXT NOT NULL DEFAULT 'post-land';",
    // 26: the CI run a `ci` schedule is following (s175, decision 0170), as text the daemon
    // reads and writes; empty when none. Additive.
    "ALTER TABLE schedules ADD COLUMN follow TEXT NOT NULL DEFAULT '';",
    // 27: what a test command did on an exact content tree (s179, decision 0174), for the
    // single-command path that has no suites: keyed by the tree and a hash of the resolved
    // command text. `green` 1: passed (reused instead of run again); 0: red with the failing
    // tests in `names`. `source` says who ran it. Additive.
    "CREATE TABLE command_runs (
         tree   TEXT NOT NULL,
         cmd    TEXT NOT NULL,
         green  INTEGER NOT NULL,
         source TEXT NOT NULL,
         names  TEXT NOT NULL,
         at     INTEGER NOT NULL,
         PRIMARY KEY (tree, cmd)
     );",
    // 28: when a task first joined the landing queue for a landing run (s184, decision 0179),
    // so a restart does not start its batch window over. Additive.
    "CREATE TABLE landing_submitted (
         task  INTEGER NOT NULL,
         run   INTEGER NOT NULL,
         at_ms INTEGER NOT NULL,
         PRIMARY KEY (task, run)
     );",
    // 29: what a suite did at a base commit beyond its failing names (s246, decision 0241): the
    // build's first error, whether the run proved nothing, whether it was red, and the first
    // error line of a red run. `complete` 1 marks a row written with all of that; an older row
    // has 0 and is worked out again when a comparison needs it. And the base-red wait of a task
    // (one row per task), so that a restarted daemon finds the wait again. Additive.
    "ALTER TABLE baselines ADD COLUMN build_error TEXT NOT NULL DEFAULT '';
     ALTER TABLE baselines ADD COLUMN vacuous INTEGER NOT NULL DEFAULT 0;
     ALTER TABLE baselines ADD COLUMN red INTEGER NOT NULL DEFAULT 0;
     ALTER TABLE baselines ADD COLUMN first_error TEXT NOT NULL DEFAULT '';
     ALTER TABLE baselines ADD COLUMN complete INTEGER NOT NULL DEFAULT 0;
     CREATE TABLE base_red (
         task       INTEGER PRIMARY KEY,
         suite      TEXT NOT NULL,
         kind       TEXT NOT NULL,
         first_error TEXT NOT NULL,
         base       TEXT NOT NULL,
         text       TEXT NOT NULL,
         at_ms      INTEGER NOT NULL
     );",
    // 30: the retries the daemon made by itself after a failure outside the code (s247), so a
    // restart neither forgets nor repeats them. Additive.
    "CREATE TABLE auto_retries (
         id     INTEGER PRIMARY KEY AUTOINCREMENT,
         task   INTEGER NOT NULL,
         at_ms  INTEGER NOT NULL,
         reason TEXT NOT NULL
     );
     CREATE INDEX auto_retries_task ON auto_retries(task, at_ms);",
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

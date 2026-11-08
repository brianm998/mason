// SPDX-License-Identifier: GPL-3.0-or-later

//! The tables of migration 16: the attempt history the detectors read, the findings and their
//! verdicts, recurring jobs, and the triage review queue.

use crate::{Store, StoreError};
use mason_core::introspect::Attempt;
use rusqlite::{params, Connection, OptionalExtension};

/// A finding that became a proposal, and what came of its fix.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FindingRow {
    pub id: u64,
    pub rule: String,
    pub subject: String,
    pub project: String,
    pub metric: String,
    pub title: String,
    /// The proposal's task.
    pub task: u64,
    pub before_num: f64,
    pub before_den: f64,
    pub before_n: u64,
    pub at: u64,
    /// `` (still open), `worked`, `didnt_help` or `unmeasured`. `pending` is never stored.
    pub verdict: String,
    pub verdict_text: String,
    pub after_num: f64,
    pub after_den: f64,
    pub after_n: u64,
    pub landed_at: u64,
    pub closed_at: u64,
}

/// A recurring job's memory.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScheduleRow {
    pub name: String,
    pub project: String,
    pub kind: String,
    pub every_ms: u64,
    pub last_run: u64,
    pub last_task: Option<u64>,
    pub last_status: String,
    /// Failures a base-health run has accepted as already there; `None` before the first run.
    pub baseline: Option<Vec<String>>,
    pub runs: u64,
}

/// One suggestion of the triage agent, kept for the owner's review.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TriageRow {
    pub id: u64,
    pub task: u64,
    pub question: String,
    /// `answer`, `close` or `escalate`: what the agent would do. It never does it.
    pub action: String,
    pub answer: String,
    pub evidence: String,
    /// `queued` (the owner reviews it) or `unsampled`, then `reviewed`.
    pub status: String,
    /// The owner's verdict on a reviewed one: `agree` or `disagree`.
    pub verdict: String,
    pub at: u64,
}

/// Insert an attempt on any connection (the planting tool uses its own).
pub fn insert_attempt(c: &Connection, a: &Attempt) -> rusqlite::Result<()> {
    c.execute(
        "INSERT INTO step_attempts(task, project, step, outcome, reason, signature, retry_n, at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        params![
            a.task as i64,
            a.project,
            a.step,
            a.outcome,
            a.reason,
            a.signature,
            a.retry_n,
            a.at as i64
        ],
    )
    .map(|_| ())
}

impl Store {
    /// Record how a step attempt ended.
    pub fn record_attempt(&self, a: &Attempt) -> Result<(), StoreError> {
        let a = a.clone();
        self.write(move |c| insert_attempt(c, &a))
    }

    /// Attempts that ended in `[from, to)`, oldest first; `project` limits them if given.
    pub fn attempts_between(
        &self,
        project: Option<&str>,
        from: u64,
        to: u64,
    ) -> Result<Vec<Attempt>, StoreError> {
        let p = project.map(String::from);
        self.read(move |c| {
            let mut st = c.prepare(
                "SELECT task, project, step, outcome, reason, signature, retry_n, at
                 FROM step_attempts
                 WHERE at >= ?1 AND at < ?2 AND (?3 IS NULL OR project = ?3) ORDER BY id",
            )?;
            let rows = st.query_map(params![from as i64, to as i64, p], |r| {
                Ok(Attempt {
                    task: r.get::<_, i64>(0)? as u64,
                    project: r.get(1)?,
                    step: r.get(2)?,
                    outcome: r.get(3)?,
                    reason: r.get(4)?,
                    signature: r.get(5)?,
                    retry_n: r.get::<_, i64>(6)? as u32,
                    at: r.get::<_, i64>(7)? as u64,
                })
            })?;
            rows.collect()
        })
    }

    /// The projects that have any attempt recorded.
    pub fn attempt_projects(&self) -> Result<Vec<String>, StoreError> {
        self.read(|c| {
            let mut st =
                c.prepare("SELECT DISTINCT project FROM step_attempts ORDER BY project")?;
            let rows = st.query_map([], |r| r.get(0))?;
            rows.collect()
        })
    }

    // ---- findings ----

    pub fn add_finding(&self, f: &FindingRow) -> Result<u64, StoreError> {
        let f = f.clone();
        self.write(move |c| {
            c.execute(
                "INSERT INTO findings(rule, subject, project, metric, title, task, before_num, before_den, before_n, at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                params![
                    f.rule, f.subject, f.project, f.metric, f.title, f.task as i64,
                    f.before_num, f.before_den, f.before_n as i64, f.at as i64
                ],
            )?;
            Ok(c.last_insert_rowid() as u64)
        })
    }

    pub fn findings(&self) -> Result<Vec<FindingRow>, StoreError> {
        self.read(|c| {
            let mut st = c.prepare(
                "SELECT id, rule, subject, project, metric, title, task, before_num, before_den, before_n, at,
                        verdict, verdict_text, after_num, after_den, after_n, landed_at, closed_at
                 FROM findings ORDER BY id",
            )?;
            let rows = st.query_map([], |r| {
                Ok(FindingRow {
                    id: r.get::<_, i64>(0)? as u64,
                    rule: r.get(1)?,
                    subject: r.get(2)?,
                    project: r.get(3)?,
                    metric: r.get(4)?,
                    title: r.get(5)?,
                    task: r.get::<_, i64>(6)? as u64,
                    before_num: r.get(7)?,
                    before_den: r.get(8)?,
                    before_n: r.get::<_, i64>(9)? as u64,
                    at: r.get::<_, i64>(10)? as u64,
                    verdict: r.get(11)?,
                    verdict_text: r.get(12)?,
                    after_num: r.get(13)?,
                    after_den: r.get(14)?,
                    after_n: r.get::<_, i64>(15)? as u64,
                    landed_at: r.get::<_, i64>(16)? as u64,
                    closed_at: r.get::<_, i64>(17)? as u64,
                })
            })?;
            rows.collect()
        })
    }

    /// Record a verdict on a finding. Only an open finding takes one; returns whether it did.
    #[allow(clippy::too_many_arguments)]
    pub fn close_finding(
        &self,
        id: u64,
        verdict: &str,
        text: &str,
        after: (f64, f64, u64),
        landed_at: u64,
        now_ms: u64,
    ) -> Result<bool, StoreError> {
        let (v, t) = (verdict.to_string(), text.to_string());
        self.write(move |c| {
            c.execute(
                "UPDATE findings SET verdict = ?2, verdict_text = ?3, after_num = ?4, after_den = ?5,
                        after_n = ?6, landed_at = ?7, closed_at = ?8
                 WHERE id = ?1 AND verdict = ''",
                params![id as i64, v, t, after.0, after.1, after.2 as i64, landed_at as i64, now_ms as i64],
            )
            .map(|n| n > 0)
        })
    }

    // ---- schedules ----

    pub fn schedule(&self, name: &str) -> Result<Option<ScheduleRow>, StoreError> {
        let n = name.to_string();
        self.read(move |c| {
            c.query_row(
                &format!("{SCHEDULE_SELECT} WHERE name = ?1"),
                params![n],
                schedule_of,
            )
            .optional()
        })
    }

    pub fn schedules(&self) -> Result<Vec<ScheduleRow>, StoreError> {
        self.read(|c| {
            let mut st = c.prepare(&format!("{SCHEDULE_SELECT} ORDER BY name"))?;
            let rows = st.query_map([], schedule_of)?;
            rows.collect()
        })
    }

    pub fn put_schedule(&self, s: &ScheduleRow) -> Result<(), StoreError> {
        let s = s.clone();
        self.write(move |c| {
            c.execute(
                "INSERT OR REPLACE INTO schedules(name, project, kind, every_ms, last_run, last_task, last_status, baseline, runs)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                params![
                    s.name, s.project, s.kind, s.every_ms as i64, s.last_run as i64,
                    s.last_task.map(|t| t as i64), s.last_status,
                    s.baseline.as_ref().map(|b| b.join("\n")), s.runs as i64
                ],
            )
            .map(|_| ())
        })
    }

    // ---- triage ----

    pub fn add_triage(&self, t: &TriageRow) -> Result<u64, StoreError> {
        let t = t.clone();
        self.write(move |c| {
            c.execute(
                "INSERT INTO triage_samples(task, question, action, answer, evidence, status, at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7)",
                params![
                    t.task as i64,
                    t.question,
                    t.action,
                    t.answer,
                    t.evidence,
                    t.status,
                    t.at as i64
                ],
            )?;
            Ok(c.last_insert_rowid() as u64)
        })
    }

    pub fn triage(&self) -> Result<Vec<TriageRow>, StoreError> {
        self.read(|c| {
            let mut st = c.prepare(
                "SELECT id, task, question, action, answer, evidence, status, verdict, at FROM triage_samples ORDER BY id",
            )?;
            let rows = st.query_map([], |r| {
                Ok(TriageRow {
                    id: r.get::<_, i64>(0)? as u64,
                    task: r.get::<_, i64>(1)? as u64,
                    question: r.get(2)?,
                    action: r.get(3)?,
                    answer: r.get(4)?,
                    evidence: r.get(5)?,
                    status: r.get(6)?,
                    verdict: r.get(7)?,
                    at: r.get::<_, i64>(8)? as u64,
                })
            })?;
            rows.collect()
        })
    }

    /// The owner reviewed a queued sample. Returns whether one was waiting.
    pub fn review_triage(&self, id: u64, agree: bool) -> Result<bool, StoreError> {
        let v = if agree { "agree" } else { "disagree" };
        self.write(move |c| {
            c.execute(
                "UPDATE triage_samples SET status = 'reviewed', verdict = ?2 WHERE id = ?1 AND status = 'queued'",
                params![id as i64, v],
            )
            .map(|n| n > 0)
        })
    }

    // ---- what the detectors look at besides attempts ----

    /// Debts made by force-skipping a suite since `since_ms`: `(suite, count)`.
    pub fn force_skips_since(
        &self,
        project: &str,
        since_ms: u64,
    ) -> Result<Vec<(String, u64)>, StoreError> {
        let p = project.to_string();
        self.read(move |c| {
            let mut st = c.prepare(
                "SELECT substr(scope, 7), COUNT(*) FROM debts
                 WHERE project = ?1 AND at >= ?2 AND scope LIKE 'suite:%' GROUP BY scope ORDER BY scope",
            )?;
            let rows = st.query_map(params![p, since_ms as i64], |r| {
                Ok((r.get(0)?, r.get::<_, i64>(1)? as u64))
            })?;
            rows.collect()
        })
    }

    /// Every forced skip or landing in `[from, to)`: `(project, task, scope, reason)`.
    pub fn debts_between(
        &self,
        from: u64,
        to: u64,
    ) -> Result<Vec<(String, u64, String, String)>, StoreError> {
        self.read(move |c| {
            let mut st = c.prepare(
                "SELECT project, task, scope, reason FROM debts WHERE at >= ?1 AND at < ?2 ORDER BY id",
            )?;
            let rows = st.query_map(params![from as i64, to as i64], |r| {
                Ok((r.get(0)?, r.get::<_, i64>(1)? as u64, r.get(2)?, r.get(3)?))
            })?;
            rows.collect()
        })
    }

    /// Overrides and quarantines past their expiry that are still stored, as readable names.
    pub fn expired_switches(&self, now_ms: u64) -> Result<Vec<String>, StoreError> {
        self.read(move |c| {
            let mut out = vec![];
            let mut st = c.prepare(
                "SELECT project, key, owner FROM overrides WHERE expires_at <= ?1 ORDER BY id",
            )?;
            for r in st.query_map(params![now_ms as i64], |r| {
                Ok(format!(
                    "override {}.{} (owner {})",
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?
                ))
            })? {
                out.push(r?);
            }
            let mut st = c.prepare(
                "SELECT suite, test, owner FROM quarantine WHERE expires_at <= ?1 ORDER BY suite, test",
            )?;
            for r in st.query_map(params![now_ms as i64], |r| {
                Ok(format!(
                    "quarantine {}::{} (owner {})",
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?
                ))
            })? {
                out.push(r?);
            }
            Ok(out)
        })
    }
}

const SCHEDULE_SELECT: &str =
    "SELECT name, project, kind, every_ms, last_run, last_task, last_status, baseline, runs FROM schedules";

fn schedule_of(r: &rusqlite::Row) -> rusqlite::Result<ScheduleRow> {
    Ok(ScheduleRow {
        name: r.get(0)?,
        project: r.get(1)?,
        kind: r.get(2)?,
        every_ms: r.get::<_, i64>(3)? as u64,
        last_run: r.get::<_, i64>(4)? as u64,
        last_task: r.get::<_, Option<i64>>(5)?.map(|t| t as u64),
        last_status: r.get(6)?,
        baseline: r
            .get::<_, Option<String>>(7)?
            .map(|b| b.lines().map(String::from).collect()),
        runs: r.get::<_, i64>(8)? as u64,
    })
}

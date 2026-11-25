// SPDX-License-Identifier: GPL-3.0-or-later

//! What pipeline self-correction did (migration 40, s432): one row per daemon start, health
//! check, finding that was handled, session and verdict.

use crate::{Store, StoreError};
use rusqlite::params;

/// `start`, `planned` (a takeover is about to exec), `cancelled` (that exec failed), `check`, `handled` (a finding a session took up), `session`, `verdict` or `action`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HealthRow {
    pub id: u64,
    pub at_ms: u64,
    pub kind: String,
    pub key: String,
    pub session: Option<u64>,
    pub detail: String,
}

impl Store {
    pub fn add_health(
        &self,
        kind: &str,
        key: &str,
        session: Option<u64>,
        detail: &str,
        now_ms: u64,
    ) -> Result<u64, StoreError> {
        let (k, y, d) = (kind.to_string(), key.to_string(), detail.to_string());
        self.write(move |c| {
            c.execute(
                "INSERT INTO health_log(at_ms, kind, key, session, detail) VALUES (?1,?2,?3,?4,?5)",
                params![now_ms as i64, k, y, session.map(|s| s as i64), d],
            )?;
            Ok(c.last_insert_rowid() as u64)
        })
    }

    /// Rows of the kinds given at or after `since_ms`, oldest first.
    pub fn health_since(
        &self,
        kinds: &[&str],
        since_ms: u64,
    ) -> Result<Vec<HealthRow>, StoreError> {
        let kinds: Vec<String> = kinds.iter().map(|k| k.to_string()).collect();
        self.read(move |c| {
            let mut st = c.prepare(
                "SELECT id, at_ms, kind, key, session, detail FROM health_log \
                 WHERE at_ms >= ?1 ORDER BY id",
            )?;
            let rows = st.query_map(params![since_ms as i64], |r| {
                Ok(HealthRow {
                    id: r.get::<_, i64>(0)? as u64,
                    at_ms: r.get::<_, i64>(1)? as u64,
                    kind: r.get(2)?,
                    key: r.get(3)?,
                    session: r.get::<_, Option<i64>>(4)?.map(|s| s as u64),
                    detail: r.get(5)?,
                })
            })?;
            let mut out = vec![];
            for r in rows {
                let r = r?;
                if kinds.is_empty() || kinds.contains(&r.kind) {
                    out.push(r);
                }
            }
            Ok(out)
        })
    }

    /// The newest `n` rows of every kind, newest first.
    pub fn health_recent(&self, n: usize) -> Result<Vec<HealthRow>, StoreError> {
        let mut v = self.health_since(&[], 0)?;
        v.reverse();
        v.truncate(n);
        Ok(v)
    }
}

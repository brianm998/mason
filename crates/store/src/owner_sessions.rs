// SPDX-License-Identifier: GPL-3.0-or-later

//! The owner's chat sessions with an agent and their messages (migration 36, s364).

use crate::{Store, StoreError};
use rusqlite::{params, Row};

/// One session. `task` is `None` for a session about no task (kind other than `task`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnerSessionRow {
    pub id: u64,
    pub task: Option<u64>,
    pub kind: String,
    pub account: String,
    /// The provider's session id, once a turn has told it ("" before the first).
    pub adapter_session: String,
    pub opened_by: String,
    pub opened_ms: u64,
    pub last_ms: u64,
    pub turns: u32,
    /// `open`, `working` or `ended`.
    pub state: String,
    pub ended_ms: u64,
}

/// One message of a session: `owner`, `agent` or `tool` (a one-line summary).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnerMessageRow {
    pub id: u64,
    pub session: u64,
    pub role: String,
    pub text: String,
    pub at_ms: u64,
}

const COLS: &str = "id, task, kind, account, adapter_session, opened_by, opened_ms, last_ms, \
                    turns, state, ended_ms";

fn session_of(r: &Row) -> rusqlite::Result<OwnerSessionRow> {
    Ok(OwnerSessionRow {
        id: r.get::<_, i64>(0)? as u64,
        task: r.get::<_, Option<i64>>(1)?.map(|t| t as u64),
        kind: r.get(2)?,
        account: r.get(3)?,
        adapter_session: r.get(4)?,
        opened_by: r.get(5)?,
        opened_ms: r.get::<_, i64>(6)? as u64,
        last_ms: r.get::<_, i64>(7)? as u64,
        turns: r.get::<_, i64>(8)? as u32,
        state: r.get(9)?,
        ended_ms: r.get::<_, i64>(10)? as u64,
    })
}

impl Store {
    /// A new open session. Returns its id.
    pub fn add_owner_session(
        &self,
        task: Option<u64>,
        kind: &str,
        account: &str,
        opened_by: &str,
        now_ms: u64,
    ) -> Result<u64, StoreError> {
        let (k, a, o) = (kind.to_string(), account.to_string(), opened_by.to_string());
        self.write(move |c| {
            c.execute(
                "INSERT INTO owner_sessions(task, kind, account, opened_by, opened_ms, last_ms, state) \
                 VALUES (?1,?2,?3,?4,?5,?5,'open')",
                params![task.map(|t| t as i64), k, a, o, now_ms as i64],
            )?;
            Ok(c.last_insert_rowid() as u64)
        })
    }

    pub fn owner_session(&self, id: u64) -> Result<Option<OwnerSessionRow>, StoreError> {
        self.read(move |c| {
            let mut st = c.prepare(&format!("SELECT {COLS} FROM owner_sessions WHERE id = ?1"))?;
            let mut rows = st.query_map(params![id as i64], session_of)?;
            rows.next().transpose()
        })
    }

    /// Every session that has not ended, oldest first.
    pub fn open_owner_sessions(&self) -> Result<Vec<OwnerSessionRow>, StoreError> {
        self.read(|c| {
            let mut st = c.prepare(&format!(
                "SELECT {COLS} FROM owner_sessions WHERE state != 'ended' ORDER BY id"
            ))?;
            let rows = st.query_map([], session_of)?;
            rows.collect()
        })
    }

    /// Set the state. Ending records when; any other state counts as activity.
    pub fn set_owner_session_state(
        &self,
        id: u64,
        state: &str,
        now_ms: u64,
    ) -> Result<(), StoreError> {
        let s = state.to_string();
        self.write(move |c| {
            c.execute(
                "UPDATE owner_sessions SET state = ?2, \
                 ended_ms = CASE WHEN ?2 = 'ended' THEN ?3 ELSE ended_ms END, \
                 last_ms = CASE WHEN ?2 = 'ended' THEN last_ms ELSE ?3 END WHERE id = ?1",
                params![id as i64, s, now_ms as i64],
            )
            .map(|_| ())
        })
    }

    /// A turn ended: the provider's session id (if it said one), one more turn, the time. A
    /// session that was `working` is `open` again; one that was ended stays ended.
    pub fn finish_owner_turn(
        &self,
        id: u64,
        adapter_session: Option<&str>,
        now_ms: u64,
    ) -> Result<(), StoreError> {
        let a = adapter_session.map(String::from);
        self.write(move |c| {
            c.execute(
                "UPDATE owner_sessions SET turns = turns + 1, last_ms = ?3, \
                 adapter_session = COALESCE(?2, adapter_session), \
                 state = CASE WHEN state = 'working' THEN 'open' ELSE state END WHERE id = ?1",
                params![id as i64, a, now_ms as i64],
            )
            .map(|_| ())
        })
    }

    /// Append a message. Returns its id.
    pub fn add_owner_message(
        &self,
        session: u64,
        role: &str,
        text: &str,
        now_ms: u64,
    ) -> Result<u64, StoreError> {
        let (r, t) = (role.to_string(), text.to_string());
        self.write(move |c| {
            c.execute(
                "INSERT INTO owner_messages(session, role, text, at_ms) VALUES (?1,?2,?3,?4)",
                params![session as i64, r, t, now_ms as i64],
            )?;
            Ok(c.last_insert_rowid() as u64)
        })
    }

    /// A session's messages after message id `after` (0: all), in order.
    pub fn owner_messages(
        &self,
        session: u64,
        after: u64,
    ) -> Result<Vec<OwnerMessageRow>, StoreError> {
        self.read(move |c| {
            let mut st = c.prepare(
                "SELECT id, session, role, text, at_ms FROM owner_messages \
                 WHERE session = ?1 AND id > ?2 ORDER BY id",
            )?;
            let rows = st.query_map(params![session as i64, after as i64], |r| {
                Ok(OwnerMessageRow {
                    id: r.get::<_, i64>(0)? as u64,
                    session: r.get::<_, i64>(1)? as u64,
                    role: r.get(2)?,
                    text: r.get(3)?,
                    at_ms: r.get::<_, i64>(4)? as u64,
                })
            })?;
            rows.collect()
        })
    }

    /// After a restart: every session that was `working` goes back to `open`, with a note in its
    /// transcript that the reply was interrupted. Returns the ids.
    pub fn interrupt_owner_sessions(&self, now_ms: u64) -> Result<Vec<u64>, StoreError> {
        self.write(move |c| {
            let ids: Vec<i64> = {
                let mut st = c.prepare("SELECT id FROM owner_sessions WHERE state = 'working'")?;
                let rows = st.query_map([], |r| r.get(0))?;
                rows.collect::<Result<_, _>>()?
            };
            for id in &ids {
                c.execute(
                    "INSERT INTO owner_messages(session, role, text, at_ms) VALUES (?1,'tool',?2,?3)",
                    params![id, "the reply was interrupted by a restart; send again", now_ms as i64],
                )?;
                c.execute(
                    "UPDATE owner_sessions SET state = 'open', last_ms = ?2 WHERE id = ?1",
                    params![id, now_ms as i64],
                )?;
            }
            Ok(ids.into_iter().map(|i| i as u64).collect())
        })
    }
}

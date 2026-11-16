// SPDX-License-Identifier: GPL-3.0-or-later

//! The table of migration 19: devices that paired with the daemon. Only the SHA-256 of a
//! token is kept, so a copy of the database does not hold working tokens.

use crate::{Store, StoreError};
use rusqlite::params;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceRow {
    pub id: u64,
    pub name: String,
    pub role: String,
    pub created_at: u64,
    pub last_seen: Option<u64>,
    pub revoked_at: Option<u64>,
}

/// A device a push can go to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PushTarget {
    pub device: u64,
    pub role: String,
    pub token: String,
    /// `sandbox` or `production`.
    pub env: String,
}

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<DeviceRow> {
    Ok(DeviceRow {
        id: r.get::<_, i64>(0)? as u64,
        name: r.get(1)?,
        role: r.get(2)?,
        created_at: r.get::<_, i64>(3)? as u64,
        last_seen: r.get::<_, Option<i64>>(4)?.map(|v| v as u64),
        revoked_at: r.get::<_, Option<i64>>(5)?.map(|v| v as u64),
    })
}

const COLS: &str = "id, name, role, created_at, last_seen, revoked_at";

impl Store {
    /// Add a device; returns its id.
    pub fn create_device(
        &self,
        name: &str,
        role: &str,
        token_sha256: &str,
        now: u64,
    ) -> Result<u64, StoreError> {
        let (name, role, hash) = (name.to_string(), role.to_string(), token_sha256.to_string());
        self.write(move |c| {
            c.execute(
                "INSERT INTO devices(name, role, token_sha256, created_at) VALUES (?1,?2,?3,?4)",
                params![name, role, hash, now as i64],
            )?;
            Ok(c.last_insert_rowid() as u64)
        })
    }

    /// The live (not revoked) device with this token hash.
    pub fn find_device(&self, token_sha256: &str) -> Result<Option<DeviceRow>, StoreError> {
        let hash = token_sha256.to_string();
        self.read(move |c| {
            let mut st = c.prepare(&format!(
                "SELECT {COLS} FROM devices WHERE token_sha256 = ?1 AND revoked_at IS NULL"
            ))?;
            let mut rows = st.query_map(params![hash], row)?;
            rows.next().transpose()
        })
    }

    pub fn touch_device(&self, id: u64, now: u64) -> Result<(), StoreError> {
        self.write(move |c| {
            c.execute(
                "UPDATE devices SET last_seen = ?2 WHERE id = ?1",
                params![id as i64, now as i64],
            )?;
            Ok(())
        })
    }

    /// Revoke a device. False when there is no such live device.
    pub fn revoke_device(&self, id: u64, now: u64) -> Result<bool, StoreError> {
        self.write(move |c| {
            let n = c.execute(
                "UPDATE devices SET revoked_at = ?2, push_token = NULL, push_env = NULL
                 WHERE id = ?1 AND revoked_at IS NULL",
                params![id as i64, now as i64],
            )?;
            Ok(n > 0)
        })
    }

    /// Record the push token of a live device. False when there is no such live device.
    pub fn set_push(&self, id: u64, token: &str, env: &str) -> Result<bool, StoreError> {
        let (token, env) = (token.to_string(), env.to_string());
        self.write(move |c| {
            let n = c.execute(
                "UPDATE devices SET push_token = ?2, push_env = ?3
                 WHERE id = ?1 AND revoked_at IS NULL",
                params![id as i64, token, env],
            )?;
            Ok(n > 0)
        })
    }

    /// Forget a device's push token.
    pub fn clear_push(&self, id: u64) -> Result<(), StoreError> {
        self.write(move |c| {
            c.execute(
                "UPDATE devices SET push_token = NULL, push_env = NULL WHERE id = ?1",
                params![id as i64],
            )?;
            Ok(())
        })
    }

    /// Forget a token Apple said is dead, unless the device has registered a new one since.
    pub fn clear_push_token(&self, id: u64, token: &str) -> Result<(), StoreError> {
        let token = token.to_string();
        self.write(move |c| {
            c.execute(
                "UPDATE devices SET push_token = NULL, push_env = NULL
                 WHERE id = ?1 AND push_token = ?2",
                params![id as i64, token],
            )?;
            Ok(())
        })
    }

    /// The live devices that have a push token, in id order.
    pub fn push_targets(&self) -> Result<Vec<PushTarget>, StoreError> {
        self.read(move |c| {
            let mut st = c.prepare(
                "SELECT id, role, push_token, push_env FROM devices
                 WHERE revoked_at IS NULL AND push_token IS NOT NULL ORDER BY id",
            )?;
            let rows = st.query_map([], |r| {
                Ok(PushTarget {
                    device: r.get::<_, i64>(0)? as u64,
                    role: r.get(1)?,
                    token: r.get(2)?,
                    env: r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                })
            })?;
            rows.collect()
        })
    }

    /// The push token of one device (for tests and `GET`-less checks).
    pub fn push_of(&self, id: u64) -> Result<Option<(String, String)>, StoreError> {
        self.read(move |c| {
            let mut st = c.prepare("SELECT push_token, push_env FROM devices WHERE id = ?1")?;
            let mut rows = st.query_map(params![id as i64], |r| {
                Ok((
                    r.get::<_, Option<String>>(0)?,
                    r.get::<_, Option<String>>(1)?,
                ))
            })?;
            Ok(rows
                .next()
                .transpose()?
                .and_then(|(t, e)| t.map(|t| (t, e.unwrap_or_default()))))
        })
    }

    pub fn list_devices(&self) -> Result<Vec<DeviceRow>, StoreError> {
        self.read(move |c| {
            let mut st = c.prepare(&format!("SELECT {COLS} FROM devices ORDER BY id"))?;
            let rows = st.query_map([], row)?;
            rows.collect()
        })
    }
}

// SPDX-License-Identifier: GPL-3.0-or-later

//! The table of the nodes migration: the Macs a coordinator is paired with. Only the
//! fingerprint of each node's certificate is kept, which is public; there is no token.

use crate::{Store, StoreError};
use rusqlite::params;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeRow {
    /// The node's stable id (it survives a rename of the host).
    pub nid: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub fingerprint: String,
    pub paired_at: u64,
}

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<NodeRow> {
    Ok(NodeRow {
        nid: r.get(0)?,
        name: r.get(1)?,
        host: r.get(2)?,
        port: r.get::<_, i64>(3)? as u16,
        fingerprint: r.get(4)?,
        paired_at: r.get::<_, i64>(5)? as u64,
    })
}

const COLS: &str = "nid, name, host, port, fingerprint, paired_at";

impl Store {
    /// Record a paired node. The same `nid` again replaces its row (a node that moved or whose
    /// certificate was replaced and was paired anew); another node holding the name loses it.
    pub fn put_node(&self, n: &NodeRow) -> Result<(), StoreError> {
        let n = n.clone();
        self.write(move |c| {
            c.execute(
                "DELETE FROM nodes WHERE name = ?2 AND nid <> ?1",
                params![n.nid, n.name],
            )?;
            c.execute(
                "INSERT OR REPLACE INTO nodes(nid, name, host, port, fingerprint, paired_at)
                 VALUES (?1,?2,?3,?4,?5,?6)",
                params![
                    n.nid,
                    n.name,
                    n.host,
                    i64::from(n.port),
                    n.fingerprint,
                    n.paired_at as i64
                ],
            )?;
            Ok(())
        })
    }

    /// Every paired node, by name.
    pub fn list_nodes(&self) -> Result<Vec<NodeRow>, StoreError> {
        self.read(|c| {
            let mut st = c.prepare(&format!("SELECT {COLS} FROM nodes ORDER BY name"))?;
            let rows = st.query_map([], row)?;
            rows.collect()
        })
    }

    /// Forget the node called `name`. `false`: there was none.
    pub fn remove_node(&self, name: &str) -> Result<bool, StoreError> {
        let name = name.to_string();
        self.write(move |c| Ok(c.execute("DELETE FROM nodes WHERE name = ?1", params![name])? > 0))
    }
}

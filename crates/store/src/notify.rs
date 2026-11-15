// SPDX-License-Identifier: GPL-3.0-or-later

//! The tables of migration 18: acknowledgements and reminders on alerts, the record of every
//! delivery attempt, and the queue of announcements for whichever device speaks.

use crate::{Store, StoreError};
use rusqlite::params;

/// One try at reaching the owner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeliveryRow {
    pub alert: String,
    pub since: u64,
    pub channel: String,
    pub delivery: String,
    pub ok: bool,
    pub detail: String,
    pub at: u64,
}

/// A sentence waiting to be said (and a sound before it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnnouncementRow {
    pub id: u64,
    pub key: String,
    pub kind: String,
    pub sentence: String,
    pub sound: bool,
    pub speech: bool,
    pub created_at: u64,
    pub taken_at: Option<u64>,
    pub taken_by: Option<String>,
}

impl AnnouncementRow {
    /// A monitor or the daemon took it and played it. An expired row was dropped unheard, and a
    /// waiting one has not been played yet.
    pub fn played(&self) -> bool {
        self.taken_at.is_some() && self.taken_by.as_deref().is_some_and(|b| b != "expired")
    }
}

impl Store {
    /// Acknowledge an alert that is still true. False when there is no such live alert.
    pub fn ack_alert(&self, key: &str, now: u64) -> Result<bool, StoreError> {
        let key = key.to_string();
        self.write(move |c| {
            let n = c.execute(
                "UPDATE alerts SET acked_at = COALESCE(acked_at, ?2) WHERE key = ?1 AND cleared_at IS NULL",
                params![key, now as i64],
            )?;
            Ok(n > 0)
        })
    }

    pub fn mark_reminded(&self, key: &str, now: u64) -> Result<(), StoreError> {
        let key = key.to_string();
        self.write(move |c| {
            c.execute(
                "UPDATE alerts SET reminded_at = COALESCE(reminded_at, ?2) WHERE key = ?1",
                params![key, now as i64],
            )?;
            Ok(())
        })
    }

    pub fn add_delivery(&self, d: &DeliveryRow) -> Result<(), StoreError> {
        let d = d.clone();
        self.write(move |c| {
            c.execute(
                "INSERT INTO deliveries(alert, since, channel, delivery, ok, detail, at) VALUES (?1,?2,?3,?4,?5,?6,?7)",
                params![d.alert, d.since as i64, d.channel, d.delivery, i64::from(d.ok), d.detail, d.at as i64],
            )?;
            Ok(())
        })
    }

    /// Every attempt for one run of an alert (the same key can come back later as a new run).
    pub fn deliveries_for(&self, alert: &str, since: u64) -> Result<Vec<DeliveryRow>, StoreError> {
        let alert = alert.to_string();
        self.read(move |c| {
            let mut st = c.prepare(
                "SELECT alert, since, channel, delivery, ok, detail, at FROM deliveries WHERE alert = ?1 AND since = ?2 ORDER BY id",
            )?;
            let rows = st.query_map(params![alert, since as i64], |r| {
                Ok(DeliveryRow {
                    alert: r.get(0)?,
                    since: r.get::<_, i64>(1)? as u64,
                    channel: r.get(2)?,
                    delivery: r.get(3)?,
                    ok: r.get::<_, i64>(4)? != 0,
                    detail: r.get(5)?,
                    at: r.get::<_, i64>(6)? as u64,
                })
            })?;
            rows.collect()
        })
    }

    /// The latest sound and speech deliveries, newest first, across all alerts.
    pub fn recent_audible_deliveries(&self, limit: usize) -> Result<Vec<DeliveryRow>, StoreError> {
        self.read(move |c| {
            let mut st = c.prepare(
                "SELECT alert, since, channel, delivery, ok, detail, at FROM deliveries WHERE channel IN ('sound','speech') ORDER BY id DESC LIMIT ?1",
            )?;
            let rows = st.query_map(params![limit as i64], |r| {
                Ok(DeliveryRow {
                    alert: r.get(0)?,
                    since: r.get::<_, i64>(1)? as u64,
                    channel: r.get(2)?,
                    delivery: r.get(3)?,
                    ok: r.get::<_, i64>(4)? != 0,
                    detail: r.get(5)?,
                    at: r.get::<_, i64>(6)? as u64,
                })
            })?;
            rows.collect()
        })
    }

    /// Queue an announcement. False when one with this key already exists: it happens once.
    pub fn enqueue_announcement(
        &self,
        key: &str,
        kind: &str,
        sentence: &str,
        sound: bool,
        speech: bool,
        now: u64,
    ) -> Result<bool, StoreError> {
        let (key, kind, sentence) = (key.to_string(), kind.to_string(), sentence.to_string());
        self.write(move |c| {
            let n = c.execute(
                "INSERT OR IGNORE INTO announcements(key, kind, sentence, sound, speech, created_at) VALUES (?1,?2,?3,?4,?5,?6)",
                params![key, kind, sentence, i64::from(sound), i64::from(speech), now as i64],
            )?;
            if n > 0 {
                if let Some((group, _)) = alert_group(&key) {
                    // Only the newest round of an alert waits: an earlier untaken one is old news.
                    let prefix = format!("{group}:");
                    c.execute(
                        "UPDATE announcements SET taken_at = ?2, taken_by = 'expired' WHERE taken_at IS NULL AND key <> ?1 AND substr(key, 1, ?3) = ?4",
                        params![key, now as i64, prefix.len() as i64, prefix],
                    )?;
                }
            }
            Ok(n > 0)
        })
    }

    /// Hand the waiting announcements, oldest first, to `client`; anything older than
    /// `stale_before` is dropped instead (nobody was there to hear it, and it is old news).
    /// `backlog` is true when nobody was listening until now (a monitor just unmuted): reminders
    /// for alerts that already played are then dropped, not replayed.
    pub fn take_announcements(
        &self,
        client: &str,
        stale_before: u64,
        limit: usize,
        now: u64,
        backlog: bool,
    ) -> Result<Vec<AnnouncementRow>, StoreError> {
        let client = client.to_string();
        self.write(move |c| {
            c.execute(
                "UPDATE announcements SET taken_at = ?1, taken_by = 'expired' WHERE taken_at IS NULL AND created_at < ?2",
                params![now as i64, stale_before as i64],
            )?;
            // Backstop: a reminder is not replayed for an alert that already played once, and an
            // alert is handed over at most once, by its newest announcement.
            let all = pending(c, usize::MAX >> 1)?;
            let mut played_groups = std::collections::HashSet::new();
            {
                let mut st = c.prepare(
                    "SELECT key FROM announcements WHERE taken_at IS NOT NULL AND taken_by <> 'expired' AND key LIKE 'alert:%'",
                )?;
                for k in st.query_map([], |r| r.get::<_, String>(0))? {
                    if let Some((g, _)) = alert_group(&k?) {
                        played_groups.insert(g);
                    }
                }
            }
            let mut newest: std::collections::HashMap<String, u64> = Default::default();
            for r in &all {
                if let Some((g, _)) = alert_group(&r.key) {
                    newest.insert(g, r.id);
                }
            }
            let mut rows = Vec::new();
            for r in all {
                let drop = match alert_group(&r.key) {
                    Some((g, label)) => {
                        newest.get(&g) != Some(&r.id)
                            || (backlog && label.starts_with("reminder") && played_groups.contains(&g))
                    }
                    None => false,
                };
                if drop {
                    c.execute(
                        "UPDATE announcements SET taken_at = ?2, taken_by = 'expired' WHERE id = ?1",
                        params![r.id as i64, now as i64],
                    )?;
                } else if rows.len() < limit {
                    rows.push(r);
                }
            }
            for r in &rows {
                c.execute(
                    "UPDATE announcements SET taken_at = ?2, taken_by = ?3 WHERE id = ?1",
                    params![r.id as i64, now as i64, client],
                )?;
            }
            Ok(rows)
        })
    }

    /// Drop a waiting announcement: what it was about cleared or was acknowledged before anyone heard it.
    pub fn expire_announcement(&self, id: u64, now: u64) -> Result<(), StoreError> {
        self.write(move |c| {
            c.execute(
                "UPDATE announcements SET taken_at = ?2, taken_by = 'expired' WHERE id = ?1 AND taken_at IS NULL",
                params![id as i64, now as i64],
            )?;
            Ok(())
        })
    }

    /// When an announcement was last taken by something that plays it (not dropped unheard).
    pub fn last_played(&self) -> Result<Option<u64>, StoreError> {
        self.read(|c| {
            let t: Option<i64> = c.query_row(
                "SELECT MAX(taken_at) FROM announcements WHERE taken_at IS NOT NULL AND taken_by <> 'expired'",
                [],
                |r| r.get(0),
            )?;
            Ok(t.map(|t| t as u64))
        })
    }

    /// Waiting announcements, oldest first, without taking them.
    pub fn pending_announcements(&self, limit: usize) -> Result<Vec<AnnouncementRow>, StoreError> {
        self.read(move |c| pending(c, limit))
    }

    pub fn announcements(&self, limit: usize) -> Result<Vec<AnnouncementRow>, StoreError> {
        self.read(move |c| {
            let mut st = c.prepare(
                "SELECT id, key, kind, sentence, sound, speech, created_at, taken_at, taken_by FROM announcements ORDER BY id DESC LIMIT ?1",
            )?;
            let rows = st.query_map(params![limit as i64], row)?;
            rows.collect()
        })
    }
}

/// An alert announcement's key is `alert:<alert key>:<since>:<label>:<flags>`. Returns the part
/// that names the alert run (everything before the label) and the label.
fn alert_group(key: &str) -> Option<(String, String)> {
    if !key.starts_with("alert:") {
        return None;
    }
    let mut it = key.rsplitn(3, ':');
    let _flags = it.next()?;
    let label = it.next()?;
    let group = it.next()?;
    Some((group.to_string(), label.to_string()))
}

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<AnnouncementRow> {
    Ok(AnnouncementRow {
        id: r.get::<_, i64>(0)? as u64,
        key: r.get(1)?,
        kind: r.get(2)?,
        sentence: r.get(3)?,
        sound: r.get::<_, i64>(4)? != 0,
        speech: r.get::<_, i64>(5)? != 0,
        created_at: r.get::<_, i64>(6)? as u64,
        taken_at: r.get::<_, Option<i64>>(7)?.map(|t| t as u64),
        taken_by: r.get(8)?,
    })
}

fn pending(c: &rusqlite::Connection, limit: usize) -> rusqlite::Result<Vec<AnnouncementRow>> {
    let mut st = c.prepare(
        "SELECT id, key, kind, sentence, sound, speech, created_at, taken_at, taken_by FROM announcements WHERE taken_at IS NULL ORDER BY id LIMIT ?1",
    )?;
    let rows = st.query_map(params![limit as i64], row)?;
    rows.collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().expect("tempdir");
        let s = Store::open(dir.path().join("s.db")).expect("store");
        (dir, s)
    }

    #[test]
    fn ack_sticks_while_the_condition_holds_and_a_return_is_new() {
        let (_dir, s) = store();
        let a = |k: &str| vec![(k.to_string(), "failed".to_string(), "t".to_string())];
        s.sync_alerts(&a("failed:1"), 10).unwrap();
        assert!(s.ack_alert("failed:1", 20).unwrap());
        s.sync_alerts(&a("failed:1"), 30).unwrap();
        assert_eq!(s.alerts(true).unwrap()[0].acked_at, Some(20));
        s.sync_alerts(&[], 40).unwrap();
        assert!(!s.ack_alert("failed:1", 50).unwrap());
        s.sync_alerts(&a("failed:1"), 60).unwrap();
        let again = &s.alerts(true).unwrap()[0];
        assert_eq!((again.since, again.acked_at), (60, None));
    }

    #[test]
    fn a_later_round_replaces_the_untaken_earlier_one_of_the_same_alert() {
        let (_dir, s) = store();
        s.enqueue_announcement(
            "alert:failed:1:5:reminder1:11",
            "failed",
            "r1",
            true,
            true,
            10,
        )
        .unwrap();
        s.enqueue_announcement(
            "alert:failed:1:5:reminder2:11",
            "failed",
            "r2",
            true,
            true,
            20,
        )
        .unwrap();
        s.enqueue_announcement(
            "alert:failed:2:5:reminder1:11",
            "failed",
            "o",
            true,
            true,
            20,
        )
        .unwrap();
        let p = s.pending_announcements(10).unwrap();
        let keys: Vec<_> = p.iter().map(|r| r.key.as_str()).collect();
        assert_eq!(
            keys,
            [
                "alert:failed:1:5:reminder2:11",
                "alert:failed:2:5:reminder1:11"
            ]
        );
    }

    #[test]
    fn an_announcement_happens_once_and_old_ones_expire() {
        let (_dir, s) = store();
        assert!(s
            .enqueue_announcement("k1", "failed", "one.", true, true, 100)
            .unwrap());
        assert!(!s
            .enqueue_announcement("k1", "failed", "one.", true, true, 200)
            .unwrap());
        s.enqueue_announcement("k2", "needs_you", "two.", true, true, 300)
            .unwrap();
        let got = s.take_announcements("a", 150, 10, 400, false).unwrap();
        assert_eq!(
            got.iter().map(|r| r.key.as_str()).collect::<Vec<_>>(),
            vec!["k2"]
        );
        assert!(s
            .take_announcements("a", 0, 10, 500, false)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn a_taken_row_and_an_expired_row_read_differently() {
        let (_dir, s) = store();
        s.enqueue_announcement("old", "failed", "old.", true, true, 100)
            .unwrap();
        s.enqueue_announcement("new", "failed", "new.", true, true, 300)
            .unwrap();
        s.enqueue_announcement("wait", "failed", "wait.", true, true, 900)
            .unwrap();
        assert_eq!(s.last_played().unwrap(), None);
        s.take_announcements("a", 200, 1, 400, false).unwrap();
        let all = s.announcements(10).unwrap();
        let by = |k: &str| all.iter().find(|r| r.key == k).unwrap().clone();
        assert!(!by("old").played(), "expired unheard");
        assert_eq!(by("old").taken_by.as_deref(), Some("expired"));
        assert!(by("new").played());
        assert!(!by("wait").played(), "still waiting");
        assert_eq!(s.last_played().unwrap(), Some(400));
    }

    #[test]
    fn deliveries_are_kept_per_run_of_an_alert() {
        let (_dir, s) = store();
        let d = |since, ok| DeliveryRow {
            alert: "k".into(),
            since,
            channel: "push".into(),
            delivery: "initial".into(),
            ok,
            detail: String::new(),
            at: 1,
        };
        s.add_delivery(&d(1, true)).unwrap();
        s.add_delivery(&d(2, false)).unwrap();
        assert_eq!(s.deliveries_for("k", 2).unwrap().len(), 1);
        assert!(!s.deliveries_for("k", 2).unwrap()[0].ok);
    }
}

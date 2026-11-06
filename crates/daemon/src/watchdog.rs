// SPDX-License-Identifier: GPL-3.0-or-later

//! Event-loop stall watchdog. An earlier pipeline froze entirely when one
//! network call ran on its loop (Part 2). This task sleeps a short while and
//! measures how late it wakes; a loop that is blocked wakes late.

use crate::daemon::Daemon;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub async fn run(d: Arc<Daemon>) {
    let tick = Duration::from_millis(50);
    loop {
        let t = Instant::now();
        tokio::time::sleep(tick).await;
        let late = t.elapsed().saturating_sub(tick);
        if late >= d.cfg.stall_threshold {
            d.record_stall(late);
        }
    }
}

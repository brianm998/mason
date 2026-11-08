// SPDX-License-Identifier: GPL-3.0-or-later

//! Plants recorded history for scripts/exit/s22.sh, into the database of a running daemon
//! (a plain connection, one insert per row, no events).
//!
//!   plant attempts DB --project P --task N --step S --outcome fail|green|red
//!         [--reason TEXT] [--count K] [--retry-from R]
//!
//! Writes K rows (default 1), stamped now, with retry numbers R, R+1, ... (default from 1).

use mason_core::introspect::{signature, Attempt};
use rusqlite::Connection;

fn main() {
    let mut args: Vec<String> = std::env::args().collect();
    args.remove(0); // the program name
    if args.first().map(String::as_str) != Some("attempts") || args.len() < 2 {
        eprintln!("usage: plant attempts DB --project P --task N --step S --outcome O [--reason T] [--count K] [--retry-from R]");
        std::process::exit(2);
    }
    let db = &args[1];
    let flag = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let need = |name: &str| flag(name).unwrap_or_else(|| panic!("{name} is required"));
    let count: u32 = flag("--count").map_or(1, |c| c.parse().expect("--count"));
    let from: u32 = flag("--retry-from").map_or(1, |c| c.parse().expect("--retry-from"));
    let reason = flag("--reason").unwrap_or_default();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_millis() as u64;
    let mut conn = Connection::open(db).expect("open");
    conn.busy_timeout(std::time::Duration::from_secs(10))
        .expect("busy timeout");
    // One transaction, so a detector never sees half of a planted history.
    let tx = conn.transaction().expect("begin");
    for i in 0..count {
        let a = Attempt {
            task: need("--task").parse().expect("--task"),
            project: need("--project"),
            step: need("--step"),
            outcome: need("--outcome"),
            signature: signature(&reason),
            reason: reason.clone(),
            retry_n: from + i,
            at: now + u64::from(i),
        };
        mason_store::introspect::insert_attempt(&tx, &a).expect("insert");
    }
    tx.commit().expect("commit");
    println!("planted {count} attempt(s)");
}

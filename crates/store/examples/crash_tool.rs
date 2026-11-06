// SPDX-License-Identifier: GPL-3.0-or-later

//! Crash-test helper for scripts/exit/s03.sh.
//!
//!   crash_tool write DB          append forever, printing "ACK <seq>" after each commit
//!   crash_tool check DB MIN_SEQ  reopen, require seq >= MIN_SEQ and a clean verify()

use mason_core::*;
use mason_store::Store;
use std::io::Write;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("write") => write(&args[2]),
        Some("check") => check(&args[2], args[3].parse().expect("MIN_SEQ")),
        _ => {
            eprintln!("usage: crash_tool write DB | check DB MIN_SEQ");
            std::process::exit(2);
        }
    }
}

fn append(s: &Store, kind: EventKind) {
    let at = Timestamp(s.last_seq().expect("seq") * 10);
    let a = s.append(Event { at, kind }).expect("append");
    let mut out = std::io::stdout().lock();
    writeln!(out, "ACK {}", a.seq).expect("stdout");
    out.flush().expect("flush");
}

fn write(db: &str) {
    let s = Store::open(db).expect("open");
    if s.last_seq().expect("seq") == 0 {
        append(
            &s,
            EventKind::ProjectRegistered {
                project: ProjectId(1),
                name: "crash".into(),
            },
        );
    }
    loop {
        append(
            &s,
            EventKind::TaskFiled {
                project: ProjectId(1),
                title: "crash task".into(),
                body: String::new(),
                priority: Priority::Normal,
                trust: TrustSource::Owner,
                explored: false,
                proposed: false,
                blockers: vec![],
                not_before: None,
            },
        );
        let task = TaskId(s.world().expect("world").tasks.len() as u64 - 1);
        append(&s, EventKind::Admitted { task, entry: None });
        for _ in 0..5 {
            append(&s, EventKind::StepSucceeded { task, next: None });
        }
    }
}

fn check(db: &str, min_seq: u64) {
    let s = Store::open(db).expect("open after crash");
    let last = s.last_seq().expect("seq");
    assert!(
        last >= min_seq,
        "lost committed events: last seq {last} < acknowledged {min_seq}"
    );
    s.verify().expect("log and projections consistent");
    println!("OK last_seq={last} (acknowledged {min_seq})");
}

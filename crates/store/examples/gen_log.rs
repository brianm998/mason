// SPDX-License-Identifier: GPL-3.0-or-later

//! Generates and times a long log, for scripts/exit/s92.sh and the stage note.
//!
//!   gen_log gen DB TASKS      project `scratch`, then TASKS landed tasks of 13 events
//!                             each with a 3 KB body, as in the live log
//!   gen_log bench DB          time Store::open (replay), world(), one append, verify()

use mason_core::*;
use mason_store::Store;
use std::time::Instant;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    match a.get(1).map(String::as_str) {
        Some("gen") => gen(&a[2], a[3].parse().expect("TASKS")),
        Some("bench") => bench(&a[2]),
        _ => {
            eprintln!("usage: gen_log gen DB TASKS | gen_log bench DB");
            std::process::exit(2);
        }
    }
}

fn ev(s: &Store, t: &mut u64, kind: EventKind) -> Vec<Effect> {
    *t += 1;
    s.append(Event {
        at: Timestamp(*t * 1000),
        kind,
    })
    .expect("append")
    .effects
}

fn gen(db: &str, tasks: u64) {
    let s = Store::open(db).expect("open");
    let mut t = 0;
    let project = ProjectId(1);
    ev(
        &s,
        &mut t,
        EventKind::ProjectRegistered {
            project,
            name: "scratch".into(),
        },
    );
    let body = "x".repeat(3000);
    for i in 0..tasks {
        let task = TaskId(i);
        ev(
            &s,
            &mut t,
            EventKind::TaskFiled {
                project,
                title: format!("generated {i}"),
                body: body.clone(),
                priority: Priority::Normal,
                trust: TrustSource::Owner,
                explored: false,
                proposed: false,
                blockers: vec![],
                not_before: None,
            },
        );
        ev(&s, &mut t, EventKind::Admitted { task, entry: None });
        let on = || "tests".to_string();
        for _ in 0..2 {
            ev(
                &s,
                &mut t,
                EventKind::WaitStarted {
                    task,
                    on: on(),
                    class: None,
                },
            );
            ev(&s, &mut t, EventKind::WaitEnded { task });
        }
        ev(&s, &mut t, EventKind::Paused { task });
        ev(&s, &mut t, EventKind::Resumed { task });
        for _ in 0..5 {
            ev(&s, &mut t, EventKind::StepSucceeded { task, next: None });
        }
    }
    println!("{} events", s.last_seq().expect("seq"));
}

fn bench(db: &str) {
    let t0 = Instant::now();
    let s = Store::open(db).expect("open");
    println!("open (replay): {:?}", t0.elapsed());
    let t0 = Instant::now();
    let w = s.world().expect("world");
    println!("world(): {:?} ({} tasks)", t0.elapsed(), w.tasks.len());
    drop(w); // a held snapshot makes the next append copy once
    let t0 = Instant::now();
    s.append(Event {
        at: Timestamp(u64::MAX / 2),
        kind: EventKind::Tick,
    })
    .expect("append");
    println!("one append: {:?}", t0.elapsed());
    let t0 = Instant::now();
    for i in 0..100 {
        s.append(Event {
            at: Timestamp(u64::MAX / 2 + i),
            kind: EventKind::Tick,
        })
        .expect("append");
    }
    println!("mean of 100 more appends: {:?}", t0.elapsed() / 100);
    let t0 = Instant::now();
    s.verify().expect("verify");
    println!("verify(): {:?}", t0.elapsed());
}

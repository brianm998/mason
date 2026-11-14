// SPDX-License-Identifier: GPL-3.0-or-later

//! Stage 11 end to end: the client protocol over one WebSocket, in both
//! encodings; resuming from a sequence number; the server-computed views and
//! their sorting; allowed actions enforced on commands; the demo daemon; and
//! the latency targets.

mod support;
use mason_core::*;
use mason_daemon::config::DaemonConfig;
use mason_daemon::views::Ctx;
use mason_daemon::Daemon;
use mason_protocol::client::Client;
use mason_protocol::view::Body;
use mason_protocol::*;
use std::sync::Arc;
use std::time::{Duration, Instant};
use support::*;

/// A demo daemon (manual stepping) with its socket served on a free port.
fn demo() -> (Arc<Daemon>, u16, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let cfg = DaemonConfig::new(
        dir.path().join("d"),
        dir.path().join("c"),
        bin("mason-step-host", "step-host"),
        bin("mason-adapters", "mason-mcp"),
    );
    std::fs::create_dir_all(&cfg.data_dir).unwrap();
    let d = Daemon::start_demo(cfg).unwrap();
    let port = serve(&d);
    (d, port, dir)
}

fn serve(d: &Arc<Daemon>) -> u16 {
    let d2 = d.clone();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let _ = rt.block_on(mason_daemon::http::serve(d2));
    });
    let info = d.cfg.info_path();
    let mut port = 0;
    wait_for("the socket to be served", 20, || {
        std::fs::read_to_string(&info)
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .and_then(|v| v["port"].as_u64())
            .map(|p| port = p as u16)
            .is_some()
    });
    port
}

/// Advance the demo; whatever the daemon pushed meanwhile is returned.
fn next_step(c: &mut Client) -> Vec<ServerMessage> {
    let (r, other) = c
        .command(raw("POST", "/demo/next"), Duration::from_secs(10))
        .unwrap();
    assert!(r.ok, "{r:?}");
    other
}

fn raw(method: &str, path: &str) -> Command {
    Command {
        id: String::new(),
        body: Some(command::Body::Raw(Raw {
            method: method.into(),
            path: path.into(),
            json: String::new(),
        })),
        actor: String::new(),
        reason: String::new(),
    }
}

/// Read messages until `pred` says stop or the time is up.
fn collect(
    c: &mut Client,
    wait: Duration,
    mut stop: impl FnMut(&[ServerMessage]) -> bool,
) -> Vec<ServerMessage> {
    let end = Instant::now() + wait;
    let mut got = vec![];
    while Instant::now() < end && !stop(&got) {
        if let Some(m) = c.recv(Duration::from_millis(100)).unwrap() {
            got.push(m);
        }
    }
    got
}

fn events_of(msgs: &[ServerMessage]) -> Vec<EventRecord> {
    msgs.iter()
        .filter_map(|m| match &m.body {
            Some(server_message::Body::Events(e)) => Some(e.events.clone()),
            _ => None,
        })
        .flatten()
        .collect()
}

fn views_of(msgs: &[ServerMessage]) -> Vec<View> {
    msgs.iter()
        .filter_map(|m| match &m.body {
            Some(server_message::Body::View(u)) => u.view.clone(),
            Some(server_message::Body::Snapshot(s)) => None.or(s.views.first().cloned()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_new_client_gets_a_snapshot_in_either_encoding() {
    for enc in [Encoding::Json, Encoding::Protobuf] {
        let (_d, port, _dir) = demo();
        let mut c = Client::connect(port, enc).unwrap();
        let w = c.hello(0, false).unwrap();
        assert_eq!(w.resume, ResumeKind::Snapshot as i32, "{enc:?}");
        assert!(w.demo && w.protocol == PROTOCOL);
        let m = c.recv(Duration::from_secs(5)).unwrap().unwrap();
        match m.body {
            Some(server_message::Body::Snapshot(s)) => {
                assert_eq!(s.views.len(), 3, "header, sidebar and home");
                assert!(s
                    .views
                    .iter()
                    .any(|v| matches!(v.body, Some(Body::Sidebar(_)))));
            }
            other => panic!("expected a snapshot, got {other:?}"),
        }
    }
}

#[test]
fn the_welcome_names_the_daemons_build_and_the_oldest_client_it_serves() {
    let (_d, port, _dir) = demo();
    let mut c = Client::connect(port, Encoding::Json).unwrap();
    let w = c.hello(0, false).unwrap();
    assert_eq!(w.version, "dev", "not started from a runtime");
    assert_eq!(w.min_protocol, mason_protocol::MIN_PROTOCOL);
}

#[test]
fn a_client_too_old_gets_a_welcome_it_can_read_then_an_explanation_and_no_views() {
    for enc in [Encoding::Json, Encoding::Protobuf] {
        let (_d, port, _dir) = demo();
        let mut c = Client::connect(port, enc).unwrap();
        let w = c.hello_as(0, 0, false).unwrap();
        assert_eq!(
            (w.min_protocol, w.protocol),
            (mason_protocol::MIN_PROTOCOL, PROTOCOL),
            "{enc:?}"
        );
        match c.recv(Duration::from_secs(5)).unwrap().map(|m| m.body) {
            Some(Some(server_message::Body::Error(e))) => {
                assert!(e.message.contains("serves protocols"), "{}", e.message)
            }
            other => panic!("expected the explanation, got {other:?}"),
        }
        // Nothing else comes: no snapshot, no views.
        assert!(matches!(
            c.recv(Duration::from_millis(500)),
            Ok(None) | Err(_)
        ));
    }
}

#[test]
fn a_client_that_reconnects_gets_only_what_it_missed() {
    let (d, port, _dir) = demo();
    let mut a = Client::connect(port, Encoding::Json).unwrap();
    a.hello(0, false).unwrap();
    collect(&mut a, Duration::from_secs(2), |m| !m.is_empty());
    // Two steps while connected: the client sees every event and remembers the last.
    let mut live = next_step(&mut a);
    live.extend(next_step(&mut a));
    live.extend(collect(&mut a, Duration::from_secs(5), |m| {
        events_of(m).len() + events_of(&live).len() >= 7
    }));
    let seen = events_of(&live);
    assert_eq!(seen.len(), 7, "2 projects, then 5 tasks filed: {seen:?}");
    let last = seen.last().unwrap().seq;
    assert!(
        seen.windows(2).all(|w| w[1].seq == w[0].seq + 1),
        "in order, no gaps"
    );
    drop(a);

    // While it is away, three more steps happen.
    for _ in 0..3 {
        d.demo_next().unwrap();
    }
    let total = d.store.last_seq().unwrap();
    assert!(total > last);

    // It comes back with its last sequence number: only the missed events.
    let mut b = Client::connect(port, Encoding::Json).unwrap();
    let w = b.hello(last, false).unwrap();
    assert_eq!(w.resume, ResumeKind::Events as i32);
    assert_eq!(w.seq, total);
    let got = collect(&mut b, Duration::from_secs(3), |m| {
        !events_of(m).is_empty() && !views_of(m).is_empty()
    });
    let missed = events_of(&got);
    assert_eq!(
        missed.first().unwrap().seq,
        last + 1,
        "starts right after what it had"
    );
    assert_eq!(missed.last().unwrap().seq, total);
    assert_eq!(
        missed.len() as u64,
        total - last,
        "exactly what it missed, nothing it had"
    );
    assert!(missed.iter().all(|e| e.seq > last));
    assert!(!views_of(&got).is_empty(), "and the views as they stand");

    // Up to date: nothing is replayed.
    let mut c = Client::connect(port, Encoding::Json).unwrap();
    let w = c.hello(total, false).unwrap();
    assert_eq!(w.resume, ResumeKind::UpToDate as i32);
    assert!(collect(&mut c, Duration::from_millis(400), |_| false).is_empty());

    // Too far ahead (the daemon's data was reset): a snapshot, not a guess.
    let mut e = Client::connect(port, Encoding::Json).unwrap();
    assert_eq!(
        e.hello(total + 500, false).unwrap().resume,
        ResumeKind::Snapshot as i32
    );
}

#[test]
fn live_changes_arrive_as_events_and_fresh_views() {
    let (d, port, _dir) = demo();
    let mut c = Client::connect(port, Encoding::Protobuf).unwrap();
    c.hello(0, false).unwrap();
    collect(&mut c, Duration::from_secs(2), |m| !m.is_empty());
    d.demo_next().unwrap(); // projects
    d.demo_next().unwrap(); // five tasks filed
    let got = collect(&mut c, Duration::from_secs(5), |m| {
        views_of(m)
            .iter()
            .any(|v| matches!(&v.body, Some(Body::Header(h)) if h.attention == 1))
    });
    let header = views_of(&got)
        .into_iter()
        .rev()
        .find_map(|v| match v.body {
            Some(Body::Header(h)) => Some(h),
            _ => None,
        })
        .unwrap();
    // The proposed task wants the owner; the counts are the server's.
    assert_eq!(header.attention, 1);
    let queued = header.counts.iter().find(|c| c.state == "queued").unwrap();
    assert_eq!(queued.count, 4);
    assert_eq!(queued.label, "queued");
    let stuck = header
        .counts
        .iter()
        .find(|c| c.state == "needs_attention")
        .unwrap();
    assert_eq!(stuck.label, "stuck", "the label is on the wire");
    assert!(header.counts.iter().all(|c| !c.label.is_empty()));
    let proposed = header
        .counts
        .iter()
        .find(|c| c.state == "proposed")
        .unwrap();
    assert!(proposed.wants_you && proposed.count == 1);
}

#[test]
fn every_entity_carries_its_allowed_actions_why_waiting_and_progress() {
    let (d, port, _dir) = demo();
    for _ in 0..5 {
        d.demo_next().unwrap();
    }
    let mut c = Client::connect(port, Encoding::Json).unwrap();
    c.hello(0, false).unwrap();
    c.subscribe(&[ViewKind::Sidebar, ViewKind::TaskDetail], "", 1)
        .unwrap();
    let got = collect(&mut c, Duration::from_secs(5), |m| {
        views_of(m)
            .iter()
            .any(|v| matches!(v.body, Some(Body::TaskDetail(_))))
    });
    let views = views_of(&got);
    let sidebar = views.iter().find_map(|v| match &v.body {
        Some(Body::Sidebar(s)) => Some(s.clone()),
        _ => None,
    });
    let sidebar = sidebar.expect("a sidebar");
    let row = |id: u64| {
        sidebar
            .sections
            .iter()
            .flat_map(|s| s.rows.iter())
            .find(|r| r.id == id)
            .cloned()
            .unwrap_or_else(|| panic!("task {id} in the sidebar"))
    };
    // #1 is in testing, waiting for a compile slot.
    let r1 = row(1);
    assert_eq!(r1.state, "testing");
    assert!(
        r1.why_waiting
            .starts_with("Waiting for compile slot: 2nd in line"),
        "{}",
        r1.why_waiting
    );
    assert!(r1.allowed_actions.contains(&"skip_test".to_string()));
    let cells: Vec<(&str, i32)> = r1
        .progress
        .iter()
        .map(|c| (c.step.as_str(), c.state))
        .collect();
    assert_eq!(
        cells,
        vec![
            ("explore", CellState::Filled as i32),
            ("develop", CellState::Filled as i32),
            ("test", CellState::Hatched as i32),
            ("land", CellState::Empty as i32),
        ],
        "filled means finished, hatched means waiting"
    );
    assert!(
        r1.progress[2].label.contains("compile slot"),
        "hatched cells say what for"
    );
    // #2 is waiting for the owner, with a card.
    let r2 = row(2);
    assert!(r2.allowed_actions.contains(&"answer".to_string()));
    // The detail view of #1 says the same.
    let detail = views.iter().find_map(|v| match &v.body {
        Some(Body::TaskDetail(t)) => Some(t.clone()),
        _ => None,
    });
    assert_eq!(detail.unwrap().row.unwrap().id, 1);
    // A proposal offers approve, reject and edit, and (s75) Add blocker, since it has not
    // started; a finished task offers nothing.
    assert_eq!(
        row(4).allowed_actions,
        vec!["approve", "reject", "edit", "block"]
    );
}

/// The task detail carries the plan parsed (`plan_view`) and as written (`plan`, for older clients).
#[test]
fn the_task_detail_carries_the_plan_readably_and_as_written() {
    for (text, readable) in [
        (
            r#"{"summary":"S","files":["a.rs"],"approach":"0. one\n1. two","acceptance_criteria":["c"],"extra_key":"x"}"#,
            true,
        ),
        ("{ not json", false),
    ] {
        let (d, port, dir) = demo();
        for _ in 0..5 {
            d.demo_next().unwrap();
        }
        let tdir = dir.path().join("d").join("t").join("1");
        std::fs::create_dir_all(&tdir).unwrap();
        std::fs::write(tdir.join("plan.json"), text).unwrap();
        let mut c = Client::connect(port, Encoding::Json).unwrap();
        c.hello(0, false).unwrap();
        c.subscribe(&[ViewKind::Sidebar, ViewKind::TaskDetail], "", 1)
            .unwrap();
        let got = collect(&mut c, Duration::from_secs(5), |m| {
            views_of(m)
                .iter()
                .any(|v| matches!(v.body, Some(Body::TaskDetail(_))))
        });
        let detail = views_of(&got)
            .iter()
            .find_map(|v| match &v.body {
                Some(Body::TaskDetail(t)) => Some(t.clone()),
                _ => None,
            })
            .expect("a task detail");
        assert_eq!(detail.plan, text);
        let pv = detail.plan_view.expect("a plan view");
        assert_eq!(pv.raw, text);
        if readable {
            assert_eq!(pv.steps, vec!["one", "two"]);
            assert_eq!(pv.step_numbers, vec!["0", "1"]);
            assert_eq!(pv.other[0].label, "Extra key");
            assert!(pv.note.is_empty());
        } else {
            assert!(!pv.note.is_empty() && pv.steps.is_empty());
        }
    }
}

#[test]
fn the_sidebar_is_in_queue_order_and_the_rest_newest_entry_first() {
    let (d, _port, _dir) = demo();
    // Register a project and file six tasks a few milliseconds apart.
    d.emit(EventKind::ProjectRegistered {
        project: ProjectId(1),
        name: "p".into(),
    })
    .unwrap();
    let filing = |n: u64| EventKind::TaskFiled {
        project: ProjectId(1),
        title: format!("task {n}"),
        body: String::new(),
        priority: Priority::Normal,
        trust: TrustSource::Owner,
        explored: false,
        proposed: false,
        blockers: vec![],
        not_before: None,
    };
    for n in 0..6 {
        d.emit(filing(n)).unwrap();
        std::thread::sleep(Duration::from_millis(5));
    }
    // Tasks 0, 1 and 2 are admitted and fail; 3, 4 and 5 stay queued.
    let entry = |t: u64| EventKind::Admitted {
        task: TaskId(t),
        entry: Some(Entry {
            def: mason_definition::builtin::builtin().hash.clone(),
            step: "explore".into(),
            state: State::Exploring,
            route: Route::Explore,
        }),
    };
    for t in 0..3 {
        d.emit(entry(t)).unwrap();
        d.emit(EventKind::StepFailed {
            task: TaskId(t),
            reason: "x".into(),
            class: None,
        })
        .unwrap();
    }
    let w = d.world().unwrap();
    let sidebar = Ctx::new(&d, &w).sidebar("");
    let ids = |state: &str| -> Vec<u64> {
        sidebar
            .sections
            .iter()
            .find(|s| s.state == state)
            .unwrap()
            .rows
            .iter()
            .map(|r| r.id)
            .collect()
    };
    assert_eq!(
        ids("queued"),
        vec![3, 4, 5],
        "active: oldest first, so anything stuck is at the top"
    );
    assert_eq!(ids("failed"), vec![2, 1, 0], "history: newest first");
    let by_state: Vec<&str> = sidebar.sections.iter().map(|s| s.state.as_str()).collect();
    assert_eq!(
        by_state,
        [
            "needs_attention",
            "queued",
            "exploring",
            "developing",
            "testing",
            "review",
            "landing",
            "proposed",
            "failed",
            "landed",
            "canceled"
        ],
        "needs you first, then lifecycle order, then history"
    );
    let collapsed: Vec<&str> = sidebar
        .sections
        .iter()
        .filter(|s| s.collapsed)
        .map(|s| s.state.as_str())
        .collect();
    assert_eq!(
        collapsed,
        ["failed", "landed", "canceled"],
        "history starts collapsed"
    );

    // --- Queued is the real queue; every other section is newest entry first.
    let filing_with = |n: u64, priority: Priority, blockers: Vec<TaskId>| EventKind::TaskFiled {
        project: ProjectId(1),
        title: format!("task {n}"),
        body: String::new(),
        priority,
        trust: TrustSource::Owner,
        explored: false,
        proposed: false,
        blockers,
        not_before: None,
    };
    let view = |d: &Daemon| {
        let w = d.world().unwrap();
        let sb = Ctx::new(d, &w).sidebar("");
        move |state: &str| -> Vec<u64> {
            sb.sections
                .iter()
                .find(|s| s.state == state)
                .unwrap()
                .rows
                .iter()
                .map(|r| r.id)
                .collect()
        }
    };
    // 6: urgent, filed last, can go. 7: urgent but blocked by 3.
    d.emit(filing_with(6, Priority::Urgent, vec![])).unwrap();
    std::thread::sleep(Duration::from_millis(5));
    d.emit(filing_with(7, Priority::Urgent, vec![TaskId(3)]))
        .unwrap();
    let q = view(&d)("queued");
    assert_eq!(
        q,
        vec![6, 3, 4, 5, 7],
        "urgent filed last goes first; equals by filing; a blocked task is below all that can go"
    );
    // Repeatedly taking the top row matches repeated next_admission, when the demo has a free session.
    {
        let w = d.world().unwrap();
        if let Some(next) = d.next_admission(&w) {
            assert_eq!(next.0, q[0], "the top row is what the daemon admits");
        }
    }
    // 8 and 9 are admitted; 9 moves into Developing first, then 8: 8 is on top.
    d.emit(filing_with(8, Priority::Normal, vec![])).unwrap();
    d.emit(filing_with(9, Priority::Normal, vec![])).unwrap();
    for t in [8, 9] {
        d.emit(entry(t)).unwrap();
    }
    for t in [9, 8] {
        d.emit(EventKind::StepSucceeded {
            task: TaskId(t),
            next: None,
        })
        .unwrap();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        view(&d)("developing"),
        vec![8, 9],
        "the one that entered Developing last is on top, whatever was filed first"
    );
    // 9 goes on to testing and is sent back: it tops Developing again.
    for k in [
        EventKind::StepSucceeded {
            task: TaskId(9),
            next: None,
        },
        EventKind::SentBack {
            task: TaskId(9),
            reason: "red".into(),
            to_step: String::new(),
        },
    ] {
        std::thread::sleep(Duration::from_millis(5));
        d.emit(k).unwrap();
    }
    assert_eq!(view(&d)("developing"), vec![9, 8], "sent back: top");
    // Failed orders by when the task failed, not when it was filed: 9 fails last.
    d.emit(EventKind::StepFailed {
        task: TaskId(9),
        reason: "x".into(),
        class: None,
    })
    .unwrap();
    assert_eq!(
        view(&d)("failed"),
        vec![9, 2, 1, 0],
        "failed: by entry, not filing"
    );
}

#[test]
fn a_command_outside_the_allowed_actions_is_refused_and_one_inside_works() {
    let (d, port, _dir) = demo();
    for _ in 0..7 {
        d.demo_next().unwrap();
    }
    let mut c = Client::connect(port, Encoding::Json).unwrap();
    c.hello(0, true).unwrap();
    let cmd = |body| Command {
        id: String::new(),
        body: Some(body),
        actor: "brian".into(),
        reason: String::new(),
    };
    // #0 has landed: nothing can be done to it.
    let (r, _) = c
        .command(
            cmd(command::Body::Cancel(TaskRef { task: 0 })),
            Duration::from_secs(5),
        )
        .unwrap();
    assert!(!r.ok && r.status == 409, "{r:?}");
    assert!(
        r.error
            .contains("cancel is not available for task #0 while it is landed"),
        "{}",
        r.error
    );
    let (r, _) = c
        .command(
            cmd(command::Body::Cancel(TaskRef { task: 99 })),
            Duration::from_secs(5),
        )
        .unwrap();
    assert_eq!((r.ok, r.status), (false, 404));
    // #1 (testing) may be cancelled, through the same handlers as the HTTP API.
    let (r, _) = c
        .command(
            cmd(command::Body::Cancel(TaskRef { task: 1 })),
            Duration::from_secs(5),
        )
        .unwrap();
    assert!(r.ok, "{r:?}");
    wait_for("the cancel to show", 10, || {
        state_of_demo(&d, 1) == State::Canceled
    });
    // A raw call reaches the rest of the API, and bad input is an error not a crash.
    let (r, _) = c
        .command(raw("GET", "/status"), Duration::from_secs(5))
        .unwrap();
    assert!(r.ok && r.json.contains("\"tasks\""), "{r:?}");
    let (r, _) = c
        .command(raw("DELETE", "/status"), Duration::from_secs(5))
        .unwrap();
    assert!(!r.ok && r.error.contains("not part of the API"), "{r:?}");
}

fn state_of_demo(d: &Arc<Daemon>, t: u64) -> State {
    d.world().unwrap().tasks[&TaskId(t)].state
}

#[test]
fn an_answer_carries_the_card_hash_over_the_socket_too() {
    let (d, port, _dir) = demo();
    for _ in 0..4 {
        d.demo_next().unwrap();
    }
    let mut c = Client::connect(port, Encoding::Json).unwrap();
    c.hello(0, true).unwrap();
    let cmd = |hash: &str, actor: &str| Command {
        id: String::new(),
        body: Some(command::Body::Answer(Answer {
            task: 2,
            answer: "1".into(),
            card_hash: hash.into(),
        })),
        actor: actor.into(),
        reason: String::new(),
    };
    let who = d.cfg.owner_names[0].clone();
    let (r, _) = c
        .command(cmd("0000000000000000", &who), Duration::from_secs(5))
        .unwrap();
    assert!(
        !r.ok && r.status == 409 && r.error.contains("stale"),
        "{r:?}"
    );
    let w = d.world().unwrap();
    let card = mason_daemon::attention::card_of(&d, &w.tasks[&TaskId(2)]).unwrap();
    let (r, _) = c
        .command(cmd(&card.1, "mallory"), Duration::from_secs(5))
        .unwrap();
    assert!(!r.ok && r.status == 403, "{r:?}");
    let (r, _) = c
        .command(cmd(&card.1, &who), Duration::from_secs(5))
        .unwrap();
    assert!(r.ok, "{r:?}");
}

#[test]
fn garbage_and_early_commands_get_an_error_not_a_crash() {
    let (_d, port, _dir) = demo();
    // A command before hello.
    let mut c = Client::connect(port, Encoding::Json).unwrap();
    c.send(&ClientMessage {
        body: Some(client_message::Body::Ping(Ping { nonce: 1 })),
    })
    .unwrap();
    match c.recv(Duration::from_secs(5)).unwrap() {
        Some(ServerMessage {
            body: Some(server_message::Body::Error(e)),
        }) => assert!(e.message.contains("hello")),
        other => panic!("{other:?}"),
    }
    // The wrong protocol version.
    let mut c = Client::connect(port, Encoding::Json).unwrap();
    c.send(&ClientMessage {
        body: Some(client_message::Body::Hello(Hello {
            protocol: 99,
            ..Default::default()
        })),
    })
    .unwrap();
    // (A Welcome first, so that a client can read what is needed; then the reason.)
    assert!(matches!(
        c.recv(Duration::from_secs(5)).unwrap(),
        Some(ServerMessage {
            body: Some(server_message::Body::Welcome(_))
        })
    ));
    match c.recv(Duration::from_secs(5)).unwrap() {
        Some(ServerMessage {
            body: Some(server_message::Body::Error(e)),
        }) => assert!(
            e.message.contains("serves protocols 1 to 1"),
            "{}",
            e.message
        ),
        other => panic!("{other:?}"),
    }
    // Ping works after hello.
    let mut c = Client::connect(port, Encoding::Protobuf).unwrap();
    c.hello(0, true).unwrap();
    c.send(&ClientMessage {
        body: Some(client_message::Body::Ping(Ping { nonce: 7 })),
    })
    .unwrap();
    assert!(matches!(
        c.recv(Duration::from_secs(5)).unwrap(),
        Some(ServerMessage {
            body: Some(server_message::Body::Pong(Pong { nonce: 7 }))
        })
    ));
}

#[test]
fn a_state_change_reaches_a_local_client_within_250_ms() {
    let (d, port, _dir) = demo();
    let mut c = Client::connect(port, Encoding::Protobuf).unwrap();
    c.hello(0, false).unwrap();
    collect(&mut c, Duration::from_secs(2), |m| !m.is_empty());
    let mut lat = vec![];
    for _ in 0..crate_steps() {
        let t0 = Instant::now();
        let d2 = d.clone();
        std::thread::spawn(move || {
            d2.demo_next();
        });
        // The change has reached the client when a view reflecting it arrives.
        collect(&mut c, Duration::from_secs(5), |m| views_of(m).len() >= 3);
        lat.push(t0.elapsed());
        std::thread::sleep(Duration::from_millis(50));
    }
    lat.sort();
    let median = lat[lat.len() / 2];
    let worst = *lat.last().unwrap();
    eprintln!("state change to client: median {median:?}, worst {worst:?}");
    assert!(median < Duration::from_millis(250), "median {median:?}");
    assert!(worst < Duration::from_millis(750), "worst {worst:?}");
}

fn crate_steps() -> usize {
    mason_daemon::demo::checkpoints()
}

#[test]
fn when_a_step_ends_the_next_task_is_the_next_one_admitted() {
    // One LLM session and two tasks: when the first lands the second is claimed.
    // Every periodic path that also admits (the sweep, the definition watcher) is
    // an hour away, so B can only be admitted by the step's end itself.
    let s = Scratch::new("test -f hello.txt", unique_script());
    let d = s.start_with(|c| {
        c.max_concurrent = 1;
        c.reload_interval = Duration::from_secs(3600);
        c.fetch_interval = Duration::from_secs(3600);
    });
    let a = file_task(&d, "first");
    let b = file_task(&d, "second");
    wait_state(&d, a, State::Landed, 90);
    wait_state(&d, b, State::Landed, 90);
    let events = d.store.read_since(0, 1000).unwrap();
    if std::env::var("MASON_TRACE").is_ok() {
        let t0 = events[0].event.at.0;
        for e in &events {
            eprintln!("+{:>5} ms  {:?}", e.event.at.0 - t0, e.event.kind);
        }
    }
    // One session: A holds it through development, and the moment A's agent
    // step ends it is B's.
    // One session: A holds it through development, and the moment A's agent
    // step ends it is B's. How long that takes depends on the machine, so the test
    // asserts the order of the log, which does not: B's admission is the next
    // admission after A's step ends, and nothing else is admitted in between.
    // (scripts/exit/s11.sh holds the measured latency to the DESIGN target.)
    let a_agent_done = events
        .iter()
        .position(|e| matches!(&e.event.kind, EventKind::StepSucceeded { task, next: Some(n) } if *task == a && n.state == State::Testing))
        .expect("A's agent step ends");
    // Printed for scripts/exit/s11.sh, which holds it to the DESIGN target; not asserted here.
    if let Some(adm_b) = events
        .iter()
        .find(|e| matches!(&e.event.kind, EventKind::Admitted { task, .. } if *task == b))
    {
        let gap = adm_b
            .event
            .at
            .0
            .saturating_sub(events[a_agent_done].event.at.0);
        eprintln!("a step ends, the next task is admitted: {gap} ms");
    }
    let admissions: Vec<(usize, TaskId)> = events
        .iter()
        .enumerate()
        .filter_map(|(i, e)| match &e.event.kind {
            EventKind::Admitted { task, .. } => Some((i, *task)),
            _ => None,
        })
        .collect();
    let after: Vec<TaskId> = admissions
        .iter()
        .filter(|(i, _)| *i > a_agent_done)
        .map(|(_, t)| *t)
        .collect();
    assert_eq!(
        after,
        vec![b],
        "B is the next and only admission after A's step ends: {admissions:?}"
    );
    assert_eq!(
        admissions.iter().map(|(_, t)| *t).collect::<Vec<_>>(),
        vec![a, b],
        "B was not admitted before A's session was free"
    );
}

#[test]
fn the_demo_script_is_deterministic_and_ends() {
    let (d, _port, _dir) = demo();
    let n = mason_daemon::demo::checkpoints();
    for i in 1..=n {
        assert_eq!(d.demo_next(), Some(i));
    }
    assert_eq!(d.demo_next(), None, "the script is over");
    let w = d.world().unwrap();
    let state = |t: u64| w.tasks[&TaskId(t)].state;
    assert_eq!(state(0), State::Landed);
    assert_eq!(state(1), State::Testing);
    assert_eq!(state(2), State::NeedsAttention);
    assert_eq!(state(3), State::Failed);
    assert_eq!(state(4), State::Proposed);
    // A demo daemon runs no work and touches nothing outside its data directory.
    assert_eq!(d.job_count(), 0);
}

/// s64: the demo daemon has accounts in several states, and the conformance
/// items for the usage page come out of them.
#[test]
fn demo_usage_has_accounts_in_several_states() {
    let (d, _port, _dir) = demo();
    let w = d.world().unwrap();
    let u = Ctx::new(&d, &w).usage();
    let by = |n: &str| u.providers.iter().find(|p| p.name == n).unwrap();
    // A subscription with percentages on both windows.
    let main = by("claude");
    assert!(main.windows.iter().all(|w| w.has_percent && w.has_reset));
    assert!(main.overage_text.contains("out_of_credits"));
    // Windows with no percentage.
    assert!(by("claude-quiet")
        .windows
        .iter()
        .all(|w| w.reported && !w.has_percent));
    // Blocked, with why and until when.
    let full = by("claude-full");
    assert_eq!(full.status, "blocked");
    assert!(
        full.blocked_text
            .starts_with("blocked: 5-hour window is full, until"),
        "{}",
        full.blocked_text
    );
    // An api account: spend against a budget, no windows.
    let api = by("api-main");
    assert!(api.windows.is_empty());
    let sp = api.spend.as_ref().unwrap();
    assert!(sp.has_spend && sp.has_budget && sp.month_usd > 3.0);
    // Codex reports nothing, so it shows nothing.
    let codex = by("codex");
    assert!(!codex.has_snapshot && codex.windows.is_empty());
    assert!(codex.tokens_week.as_ref().unwrap().by_model.is_empty());
    // The top bar: the most constrained window per account.
    assert!(
        u.top_bar
            .iter()
            .any(|l| l.starts_with("claude 5-hour 82%, resets 1 h")),
        "{:?}",
        u.top_bar
    );
    assert!(
        u.top_bar.iter().any(|l| l.starts_with("api-main spend $")),
        "{:?}",
        u.top_bar
    );
    // The conformance items.
    let items = mason_conformance::conformance::usage_items(&u);
    let ids: Vec<&str> = items.iter().map(|i| i.id.as_str()).collect();
    for id in [
        "usage:claude",
        "usage-window:claude:five_hour",
        "usage-window:claude-quiet:seven_day",
        "usage:api-main",
    ] {
        assert!(ids.contains(&id), "{id} in {ids:?}");
    }
    let q = items
        .iter()
        .find(|i| i.id == "usage-window:claude-quiet:five_hour")
        .unwrap();
    assert!(q.texts.contains(&"no percentage reported".to_string()));
}

/// Fill `pool` with work that waits for `gate`, until it refuses more.
fn clog(pool: &mason_daemon::pool::Pool, gate: &Arc<std::sync::Mutex<()>>) {
    let mut n = 0;
    loop {
        let g = gate.clone();
        match pool.spawn(move || drop(g.lock())) {
            Ok(()) => n += 1,
            Err(_) => break,
        }
        assert!(n < 10_000);
    }
}

fn snapshot_views(m: &[ServerMessage]) -> Option<usize> {
    m.iter().find_map(|m| match &m.body {
        Some(server_message::Body::Snapshot(s)) => Some(s.views.len()),
        _ => None,
    })
}

#[test]
fn a_full_main_pool_does_not_blank_a_new_client_or_stop_updates() {
    let (d, port, _dir) = demo();
    let gate = Arc::new(std::sync::Mutex::new(()));
    let held = gate.lock().unwrap();
    clog(&d.pool, &gate);
    std::thread::sleep(Duration::from_millis(300));
    clog(&d.pool, &gate);
    assert!(d.pool.spawn(|| {}).is_err(), "the main pool is full");
    let mut c = Client::connect(port, Encoding::Protobuf).unwrap();
    c.hello(0, false).unwrap();
    let got = collect(&mut c, Duration::from_secs(5), |m| !m.is_empty());
    assert_eq!(snapshot_views(&got), Some(3), "a full snapshot: {got:?}");
    d.demo_next().unwrap();
    d.demo_next().unwrap();
    let got = collect(&mut c, Duration::from_secs(5), |m| {
        views_of(m)
            .iter()
            .any(|v| matches!(&v.body, Some(Body::Header(h)) if h.attention == 1))
    });
    assert!(!views_of(&got).is_empty(), "a view update arrived");
    drop(held);
}

#[test]
fn views_owed_while_the_view_pool_is_busy_arrive_when_it_clears_without_another_event() {
    let (d, port, _dir) = demo();
    let mut c = Client::connect(port, Encoding::Protobuf).unwrap();
    c.hello(0, false).unwrap();
    collect(&mut c, Duration::from_secs(2), |m| !m.is_empty());
    let gate = Arc::new(std::sync::Mutex::new(()));
    let held = gate.lock().unwrap();
    clog(&d.view_pool, &gate);
    d.demo_next().unwrap();
    d.demo_next().unwrap();
    // Nothing can be computed yet.
    let got = collect(&mut c, Duration::from_millis(800), |m| {
        !views_of(m).is_empty()
    });
    assert!(views_of(&got).is_empty(), "nothing while the pool is busy");
    drop(held);
    // No further event: the session pushes what it owes by itself.
    let got = collect(&mut c, Duration::from_secs(5), |m| {
        views_of(m)
            .iter()
            .any(|v| matches!(&v.body, Some(Body::Header(h)) if h.attention == 1))
    });
    assert!(
        views_of(&got)
            .iter()
            .any(|v| matches!(&v.body, Some(Body::Header(h)) if h.attention == 1)),
        "the owed update arrived: {got:?}"
    );
}

#[test]
fn a_snapshot_waits_for_the_view_pool_and_is_never_empty() {
    let (d, port, _dir) = demo();
    let gate = Arc::new(std::sync::Mutex::new(()));
    let (g2, tx) = (gate.clone(), std::sync::mpsc::channel::<()>());
    let releaser = std::thread::spawn(move || {
        let _held = g2.lock().unwrap();
        tx.0.send(()).unwrap();
        std::thread::sleep(Duration::from_millis(1200));
    });
    tx.1.recv().unwrap();
    clog(&d.view_pool, &gate);
    let mut c = Client::connect(port, Encoding::Protobuf).unwrap();
    c.hello(0, false).unwrap();
    let got = collect(&mut c, Duration::from_secs(6), |m| !m.is_empty());
    releaser.join().unwrap();
    assert_eq!(snapshot_views(&got), Some(3), "{got:?}");
}

#[test]
fn a_snapshot_that_cannot_be_computed_closes_the_socket_instead_of_going_out_empty() {
    let (d, port, _dir) = demo();
    let gate = Arc::new(std::sync::Mutex::new(()));
    let held = gate.lock().unwrap();
    clog(&d.view_pool, &gate);
    std::thread::sleep(Duration::from_millis(300));
    clog(&d.view_pool, &gate);
    let mut c = Client::connect(port, Encoding::Protobuf).unwrap();
    // The welcome needs the view pool too, so it may never come: either way,
    // the client sees no snapshot, only a close.
    let hello = c.hello(0, false);
    let _ = hello;
    let end = Instant::now() + Duration::from_secs(9);
    let mut all = vec![];
    let mut closed = false;
    while Instant::now() < end {
        match c.recv(Duration::from_millis(200)) {
            Ok(Some(m)) => all.push(m),
            Ok(None) => {}
            Err(_) => {
                closed = true;
                break;
            }
        }
    }
    drop(held);
    assert!(snapshot_views(&all).is_none(), "no snapshot, empty or not");
    assert!(closed, "the socket was closed");
}

/// Hold `n` threads of `pool` on `gate` (and wait until they have all started).
fn hold_threads(pool: &mason_daemon::pool::Pool, gate: &Arc<std::sync::Mutex<()>>, n: usize) {
    for _ in 0..n {
        let g = gate.clone();
        pool.spawn(move || drop(g.lock())).unwrap();
    }
    wait_for("the pool threads to start", 20, || pool.queue_depth() == 0);
}

fn pong_of(m: &[ServerMessage], nonce: u64) -> bool {
    m.iter()
        .any(|m| matches!(&m.body, Some(server_message::Body::Pong(p)) if p.nonce == nonce))
}

fn has_result(m: &[ServerMessage]) -> bool {
    m.iter()
        .any(|m| matches!(&m.body, Some(server_message::Body::Result(_))))
}

#[test]
fn a_blocked_command_does_not_stop_pings_or_a_new_task_detail_view() {
    let (d, port, _dir) = demo();
    for _ in 0..5 {
        d.demo_next().unwrap();
    }
    let mut c = Client::connect(port, Encoding::Protobuf).unwrap();
    c.hello(0, false).unwrap();
    collect(&mut c, Duration::from_secs(5), |m| !m.is_empty());
    // The command waits in the blocking pool; two of the view pool's threads are held too.
    let gate = Arc::new(std::sync::Mutex::new(()));
    let held = gate.lock().unwrap();
    hold_threads(&d.pool, &gate, 4);
    hold_threads(&d.view_pool, &gate, 2);
    let cancel = Command {
        id: "slow".into(),
        body: Some(command::Body::Cancel(TaskRef { task: 1 })),
        actor: "brian".into(),
        reason: String::new(),
    };
    c.send(&ClientMessage {
        body: Some(client_message::Body::Command(cancel)),
    })
    .unwrap();
    c.send(&ClientMessage {
        body: Some(client_message::Body::Ping(Ping { nonce: 77 })),
    })
    .unwrap();
    c.subscribe(&[ViewKind::Header, ViewKind::Sidebar], "", 0)
        .unwrap();
    c.subscribe(
        &[ViewKind::Header, ViewKind::Sidebar, ViewKind::TaskDetail],
        "",
        1,
    )
    .unwrap();
    let got = collect(&mut c, Duration::from_secs(10), |m| {
        pong_of(m, 77)
            && views_of(m)
                .iter()
                .any(|v| matches!(v.body, Some(Body::TaskDetail(_))))
    });
    assert!(pong_of(&got, 77), "pong while the command waits: {got:?}");
    assert!(
        views_of(&got)
            .iter()
            .any(|v| matches!(v.body, Some(Body::TaskDetail(_)))),
        "task detail arrived: {got:?}"
    );
    assert!(!has_result(&got), "the command is still blocked");
    drop(held);
    let got = collect(&mut c, Duration::from_secs(10), has_result);
    assert!(
        got.iter()
            .any(|m| matches!(&m.body, Some(server_message::Body::Result(r)) if r.id == "slow")),
        "the result comes once released: {got:?}"
    );
}

#[test]
fn a_burst_of_ticks_queues_at_most_one_rebuild_for_a_session() {
    let (d, port, _dir) = demo();
    let mut c = Client::connect(port, Encoding::Protobuf).unwrap();
    c.hello(0, false).unwrap();
    collect(&mut c, Duration::from_secs(5), |m| !m.is_empty());
    let gate = Arc::new(std::sync::Mutex::new(()));
    let held = gate.lock().unwrap();
    hold_threads(&d.view_pool, &gate, 4);
    d.demo_next().unwrap();
    // The first tick queues one rebuild behind the held threads.
    wait_for("a rebuild to queue", 20, || d.view_pool.queue_depth() >= 1);
    for _ in 0..10 {
        d.demo_next().unwrap();
    }
    collect(&mut c, Duration::from_millis(500), |_| false);
    assert_eq!(
        d.view_pool.queue_depth(),
        1,
        "one rebuild, however many ticks"
    );
    drop(held);
}

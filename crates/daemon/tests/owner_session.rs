// SPDX-License-Identifier: GPL-3.0-or-later

//! Owner sessions (s364, decision 0357): the owner's chat with an agent, held by the daemon.
//! Turns are driven by events: a gated turn waits for a file the test creates, never for a time.

mod support;
use mason_daemon::attention;
use mason_daemon::usersession as us;
use mason_daemon::views::Ctx;
use mason_daemon::Daemon;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use support::*;

fn ask_turn() -> Value {
    json!([{"call": {"tool": "ask", "args": {
        "ask": "Should the token expire after one hour or one day?",
        "why": "The ticket does not say and the security notes are silent on it.",
        "options": [
            {"label": "One hour, with refresh tokens", "recommended": true},
            {"label": "One day, without refresh tokens"}
        ],
        "evidence": ["the ticket only says 'tokens expire'"],
        "key": ""
    }}}])
}

fn gated(gate: &Path) -> Value {
    json!([
        {"sh": format!("while [ ! -f {} ]; do sleep 0.05; done", gate.display())},
        {"say": "back from the gate"}
    ])
}

fn start(s: &Scratch) -> Arc<Daemon> {
    s.start_with(|c| c.owner_names = vec!["brian".to_string()])
}

fn open(d: &Arc<Daemon>, t: mason_core::TaskId) -> us::Opened {
    match us::open_for_task(d, t, "brian", true) {
        Ok(o) => o,
        Err(_) => panic!("the session did not open"),
    }
}

fn wait_idle(d: &Arc<Daemon>, id: u64, turns: u32) {
    wait_for("the reply to finish", 60, || {
        d.store
            .owner_session(id)
            .unwrap()
            .is_some_and(|s| s.state == "open" && s.turns >= turns)
    });
}

fn roles(d: &Arc<Daemon>, id: u64) -> Vec<(String, String)> {
    d.store
        .owner_messages(id, 0)
        .unwrap()
        .into_iter()
        .map(|m| (m.role, m.text))
        .collect()
}

fn argv(s: &Scratch) -> Vec<String> {
    std::fs::read_to_string(format!("{}.argv", s.script.display()))
        .unwrap_or_default()
        .lines()
        .map(String::from)
        .collect()
}

fn chat_turns(gate: &Path) -> Value {
    json!([
        [{"say": "I looked."}, {"tool": {"name": "Read", "input": {"file_path": "a.rs"}}}, {"say": "All fine."}],
        [{"say": "second reply"}],
        gated(gate)
    ])
}

fn gate_dir() -> (tempfile::TempDir, PathBuf) {
    let g = tempfile::Builder::new()
        .prefix("gate")
        .tempdir_in("/tmp")
        .unwrap();
    let p = g.path().join("open");
    (g, p)
}

#[test]
fn a_session_keeps_its_transcript_resumes_the_agent_and_ends() {
    let (_g, gate) = gate_dir();
    let s = Scratch::new(
        "test -f hello.txt",
        json!({"steps": {"explore": [explore_turn()], "develop": [ask_turn(), develop_turn()],
                         "chat": chat_turns(&gate)}}),
    );
    let d = start(&s);
    let t = file_task_as(
        &d,
        "Asks",
        "Please do it.",
        true,
        mason_core::TrustSource::Owner,
        false,
    );
    wait_state(&d, t, mason_core::State::NeedsAttention, 60);
    let before = argv(&s).len();

    let o = open(&d, t);
    assert!(o.created);
    let draft = o.draft.clone().expect("a draft");
    assert!(
        draft.contains("Asks") && draft.contains("Please do it."),
        "{draft}"
    );
    let id = o.session.id;
    // Opening again returns the same session.
    assert_eq!(open(&d, t).session.id, id);

    us::post(&d, id, "what did you find?", "brian").unwrap();
    wait_idle(&d, id, 1);
    let m = roles(&d, id);
    let r: Vec<&str> = m.iter().map(|(r, _)| r.as_str()).collect();
    assert_eq!(r, ["owner", "agent", "tool", "agent"], "{m:?}");
    assert_eq!(m[0].1, "what did you find?");
    assert_eq!(m[1].1, "I looked.");
    assert!(m[2].1.starts_with("Read"), "{m:?}");
    // The task agent's own session was resumed, not a new one started.
    let a = argv(&s);
    assert_eq!(a.len(), before + 1, "{a:?}");
    assert!(a[before].contains("--resume fake-claude-session"), "{a:?}");

    // The second message resumes the session the first turn reported.
    us::post(&d, id, "and now?", "brian").unwrap();
    wait_idle(&d, id, 2);
    assert_eq!(roles(&d, id).last().unwrap().1, "second reply");
    assert!(argv(&s)[before + 1].contains("--resume"), "{:?}", argv(&s));

    // The views carry the session, and the header counts it.
    let w = d.world().unwrap();
    let ctx = Ctx::new(&d, &w);
    let info = ctx.row(&w.tasks[&t]).owner_session.expect("owner_session");
    assert_eq!((info.id, info.turns, info.working), (id, 2, false));
    assert_eq!(ctx.header("").open_owner_sessions, 1);

    // "Closing the dialog" is no call at all: the session is still listed, with its transcript.
    assert_eq!(d.store.open_owner_sessions().unwrap().len(), 1);
    assert_eq!(roles(&d, id).len(), 6);
    // Reopening gives no draft: the owner has written already.
    assert!(open(&d, t).draft.is_none());

    // A restart with a turn in flight marks it interrupted and loses nothing.
    d.store.set_owner_session_state(id, "working", 1).unwrap();
    d.store.interrupt_owner_sessions(2).unwrap();
    assert_eq!(d.store.owner_session(id).unwrap().unwrap().state, "open");
    assert!(roles(&d, id).last().unwrap().1.contains("interrupted"));

    // An idle session ends by itself after idle_end_h; an owner's end is final.
    assert!(us::end_idle(&d, 1).is_empty());
    us::end(&d, id, "brian").unwrap();
    assert!(d.store.open_owner_sessions().unwrap().is_empty());
    let w = d.world().unwrap();
    let ctx = Ctx::new(&d, &w);
    assert!(ctx.row(&w.tasks[&t]).owner_session.is_none());
    assert_eq!(ctx.header("").open_owner_sessions, 0);
    assert!(us::post(&d, id, "still there?", "brian").is_err());
    // The transcript is kept.
    assert_eq!(roles(&d, id).len(), 7);

    let id2 = open(&d, t).session.id;
    assert_ne!(id2, id);
    let ended = us::end_idle(&d, mason_daemon::daemon::now_ms() + 25 * 3_600_000);
    assert_eq!(ended, vec![id2]);
}

#[test]
fn the_next_step_waits_while_a_turn_is_in_flight_and_goes_on_when_it_finishes() {
    let (_g, gate) = gate_dir();
    let s = Scratch::new(
        "test -f hello.txt",
        json!({"steps": {"explore": [explore_turn()], "develop": [ask_turn(), develop_turn()],
                         "chat": [gated(&gate)]}}),
    );
    let d = start(&s);
    let t = file_task_as(
        &d,
        "Asks",
        "Please do it.",
        true,
        mason_core::TrustSource::Owner,
        false,
    );
    wait_state(&d, t, mason_core::State::NeedsAttention, 60);
    let id = open(&d, t).session.id;
    us::post(&d, id, "look at this", "brian").unwrap();
    assert_eq!(d.store.owner_session(id).unwrap().unwrap().state, "working");

    let (_, hash) = attention::card_of(&d, &task_of(&d, t)).unwrap();
    attention::answer(&d, t, "1", &hash, "brian").unwrap();
    // The task goes on to its step, which waits and says why.
    wait_for("the hold", 60, || {
        let w = d.world().unwrap();
        Ctx::new(&d, &w).row(&w.tasks[&t]).why_waiting == us::HOLD_REASON
    });
    let launched = argv(&s).len();
    assert!(
        us::replying(&d, t),
        "the turn is still in flight at the hold"
    );
    // The turn finishes (the gate opens): the step launches and the task lands.
    std::fs::write(&gate, "").unwrap();
    wait_idle(&d, id, 1);
    wait_state(&d, t, mason_core::State::Landed, 90);
    assert!(argv(&s).len() > launched);
    // The session is still open, idle, and held nothing.
    assert_eq!(d.store.open_owner_sessions().unwrap().len(), 1);
}

fn provider_flow(codex: bool) {
    let name = if codex { "codex" } else { "claude" };
    let script = json!({"steps": {"chat": [
        [{"say": format!("first from {name}")}],
        [{"say": format!("second from {name}")}]
    ]}});
    let s = Scratch::new("true", script.clone());
    if codex {
        std::fs::write(
            s.config_dir.join("projects/scratch.toml"),
            format!(
                "path = \"{}\"\nbase = \"main\"\nremote = \"origin\"\ntest = \"true\"\n\
                 provider = \"codex\"\ncodex_bin = \"{}\"\nstep_timeout_s = 120\ntest_timeout_s = 60\n\
                 [agent_env]\nFAKE_CODEX_SCRIPT = \"{}\"\n",
                s.repo.display(),
                bin("mason-adapters", "fake-codex").display(),
                s.script.display()
            ),
        )
        .unwrap();
    }
    let d = start(&s);
    // A task that has run no agent yet: nothing to resume.
    let t = file_task_as(
        &d,
        "Quiet",
        "Body.",
        false,
        mason_core::TrustSource::Owner,
        true,
    );
    let pid = d.project_id_by_name("scratch").unwrap();
    let wt = d.project(pid).unwrap().worktree(&d.cfg, t.0);
    std::fs::create_dir_all(&wt).unwrap();
    let id = open(&d, t).session.id;
    us::post(&d, id, "hello", "brian").unwrap();
    wait_idle(&d, id, 1);
    us::post(&d, id, "again", "brian").unwrap();
    wait_idle(&d, id, 2);
    let m = roles(&d, id);
    let r: Vec<&str> = m.iter().map(|(r, _)| r.as_str()).collect();
    assert_eq!(r, ["owner", "agent", "owner", "agent"], "{m:?}");
    assert_eq!(m[1].1, format!("first from {name}"));
    assert_eq!(m[3].1, format!("second from {name}"));
    let a = argv(&s);
    assert_eq!(a.len(), 2, "{a:?}");
    assert!(
        !a[0].contains("resume"),
        "a fresh session was started: {a:?}"
    );
    assert!(a[1].contains("resume"), "the second turn resumed it: {a:?}");
    us::end(&d, id, "brian").unwrap();
    assert!(d.store.open_owner_sessions().unwrap().is_empty());
}

#[test]
fn a_task_with_no_agent_session_starts_one_and_the_next_turn_resumes_it_claude() {
    provider_flow(false);
}

#[test]
fn the_same_through_codex_nothing_is_claude_specific() {
    provider_flow(true);
}

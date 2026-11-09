// SPDX-License-Identifier: GPL-3.0-or-later

//! s76 end to end: a running agent step's cell gains the tokens its session has used
//! before the step finishes, a restart mid-step neither loses nor doubles them, and the
//! `stream_event` lines the CLI writes with `--include-partial-messages` are not output.

mod support;
use mason_core::*;
use serde_json::json;
use support::*;

/// A develop turn that says two messages (with a pause after each) before it commits.
fn slow_develop() -> serde_json::Value {
    let mut turn = vec![
        json!({"say": "first"}),
        json!({"sleep_ms": 1500}),
        json!({"say": "second"}),
        json!({"sleep_ms": 9000}),
    ];
    turn.extend(develop_turn().as_array().unwrap().iter().cloned());
    json!({"steps": {"explore": [explore_turn()], "develop": [turn]}})
}

fn live_tokens(
    d: &std::sync::Arc<mason_daemon::Daemon>,
    t: TaskId,
) -> Option<mason_core::progress::Tokens> {
    d.progress_of_task(t).iter().find_map(|p| p.token_total())
}

#[test]
fn a_running_steps_cell_gains_tokens_before_it_ends() {
    let s = Scratch::new("test -f hello.txt", slow_develop());
    let d = s.start();
    let t = file_task(&d, "Live tokens");
    // While the step is still running, the tokens the session has used are in its cell.
    let mut seen = None;
    wait_for("tokens in the running step's cell", 60, || {
        if state_of(&d, t) == State::Landed {
            panic!("the step ended before its tokens were seen");
        }
        seen = live_tokens(&d, t).filter(|k| k.output.is_some_and(|o| o >= 25));
        seen.is_some()
    });
    let seen = seen.unwrap();
    // One message is input 2, cache read 1000, cache write 50, output 25.
    assert_eq!(seen.input.unwrap() % 2, 0);
    assert!(
        seen.output.unwrap() >= 25 && seen.output.unwrap() <= 50,
        "{seen:?}"
    );
    // The cell the monitors get carries it, with the output figures free of stream events.
    let p = d.progress_of_task(t).pop().unwrap();
    let cell = mason_daemon::progress::cell_of(&p, mason_protocol::CellState::Part, "running");
    let k = cell.tokens.expect("the cell has tokens");
    assert!(k.has_input && k.has_output && k.has_cache_read && k.has_cache_write);
    assert!(!k.has_cost, "a subscription account has no cost");
    assert!(
        !p.partial
            .values()
            .any(|b| b.starts_with(b"{\"type\":\"stream_event\"")),
        "stream events are not kept as output"
    );
    wait_state(&d, t, State::Landed, 90);
}

/// The tokens of the task's running step, as `mason task show --json` has them.
fn shown_tokens(s: &Scratch) -> Option<(u64, u64)> {
    let o = s.mason(&["task", "show", "0", "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).ok()?;
    let t = v["progress"]["runs"]
        .as_array()?
        .iter()
        .find_map(|r| r["tokens"].as_object())?;
    Some((t["input"].as_u64()?, t["output"].as_u64()?))
}

#[test]
fn a_restart_in_the_middle_of_the_step_keeps_the_total_and_does_not_double_it() {
    let s = Scratch::new("test -f hello.txt", slow_develop());
    let mut m = s.masond(&[], &[]);
    s.wait_daemon(&mut m);
    s.mason_ok(&["task", "add", "Restart tokens"]);
    s.wait_task(0, "developing", 60);
    wait_for("both messages' tokens", 60, || {
        shown_tokens(&s).is_some_and(|(i, o)| i == 4 && o == 50)
    });
    // The record is written to the store every few seconds; let it be.
    std::thread::sleep(std::time::Duration::from_millis(
        mason_daemon::progress::PERSIST_MS + 1500,
    ));
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(m.child.id() as i32),
        nix::sys::signal::Signal::SIGKILL,
    )
    .unwrap();
    let _ = m.child.wait();
    let mut m2 = s.masond(&[], &[]);
    s.wait_daemon(&mut m2);
    // From the first moment the restarted daemon shows a figure it is the same one, and
    // never more, while the journal is replayed.
    let mut shown = false;
    wait_for("the step to end", 120, || {
        if let Some(k) = shown_tokens(&s) {
            assert_eq!(k, (4, 50), "after the restart");
            shown = true;
        }
        let o = s.mason(&["task", "show", "0", "--json"]);
        let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap_or_default();
        v["state"] != "developing" && v["state"] != "exploring"
    });
    assert!(shown, "the restarted daemon never showed the tokens");
    drop(m2);
    kill_hosts(&s.data_dir);
}

#[test]
fn stream_event_lines_do_not_change_the_output_counts() {
    let s = Scratch::new("test -f hello.txt", slow_develop());
    let d = s.start();
    let t = file_task(&d, "Output counts");
    wait_for("some output", 60, || {
        d.progress_of_task(t).iter().any(|p| p.output.lines > 0)
    });
    let p = d.progress_of_task(t).pop().unwrap();
    // Each message writes five stream_event lines (start, block start, three deltas, block
    // stop, delta, stop) around its assistant line; none of them is counted.
    let counted: Vec<String> = p.pending_log.clone();
    assert!(
        counted.iter().all(|l| !l.contains("stream_event")),
        "{counted:?}"
    );
    assert!(p.output.lines < 12, "{} lines", p.output.lines);
    wait_state(&d, t, State::Landed, 90);
}

/// A develop turn that keeps the session open for a while after its result, so the
/// `result` (the only event carrying a cost) is seen while the step still runs.
fn lingering_develop() -> serde_json::Value {
    let mut turn: Vec<serde_json::Value> = develop_turn().as_array().unwrap().to_vec();
    turn.insert(0, json!({"say": "one"}));
    turn.push(json!({"linger_after_result_ms": 8000}));
    json!({"steps": {"explore": [explore_turn()], "develop": [turn]}})
}

fn cost_seen(account: &str, providers: &str) -> Option<bool> {
    let s = Scratch::new("test -f hello.txt", lingering_develop());
    if !providers.is_empty() {
        std::fs::write(s.config_dir.join("providers.toml"), providers).unwrap();
        s.write_project(
            "scratch",
            "test -f hello.txt",
            &format!("account = \"{account}\""),
        );
    }
    let d = s.start();
    let t = file_task(&d, "Cost");
    wait_for(
        "both messages' figures in the running step's cell",
        60,
        || live_tokens(&d, t).is_some_and(|k| k.input == Some(4) && k.output == Some(50)),
    );
    // The session writes its result and then lingers: wait until it is in a host's journal
    // (however slow the machine is), and give the daemon a moment to read it.
    let hosts = s.data_dir.join("h");
    wait_for("the session's result", 60, || {
        std::fs::read_dir(&hosts).is_ok_and(|rd| {
            // The explore step's session wrote one too; this is the second.
            rd.flatten()
                .filter(|e| {
                    std::fs::read_to_string(e.path().join("journal"))
                        .is_ok_and(|j| j.contains("\"type\":\"result\""))
                })
                .count()
                >= 2
        })
    });
    std::thread::sleep(std::time::Duration::from_millis(1500));
    let out = d
        .progress_of_task(t)
        .iter()
        .find(|p| p.step == "develop")
        .and_then(|p| p.token_total())
        .map(|k| k.cost_nano_usd.is_some());
    wait_state(&d, t, State::Landed, 90);
    out
}

#[test]
fn cost_is_in_the_cell_for_an_api_account_and_not_for_a_subscription() {
    assert_eq!(
        cost_seen("claude", ""),
        Some(false),
        "a subscription account has no cost"
    );
    assert_eq!(
        cost_seen(
            "apiacct",
            "[provider.apiacct]\nadapter = \"claude\"\nbilling = \"api\"\n"
        ),
        Some(true),
        "an api account has it"
    );
}

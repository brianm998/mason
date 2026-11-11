// SPDX-License-Identifier: GPL-3.0-or-later

//! The dropped-line audit end to end, in one process: the real daemon, real git, real step
//! hosts, a bare origin, and the scripted stand-in for the agent CLI.
//!
//! The base lands a line in a function while the task is being developed (as another task
//! would). The task's resolution of the merge takes its own side and loses that line;
//! landing sends it back naming the line; the agent restores it; it lands.

mod support;
use mason_core::*;
use serde_json::json;
use support::*;

#[test]
fn a_resolution_that_took_its_own_side_is_sent_back_naming_the_line_and_then_lands() {
    let base_moves = "tmp=$(mktemp -d) && git worktree add -q --detach \"$tmp\" origin/main && \
        (cd \"$tmp\" && printf 'fn run() {\\n    setup();\\n    landed_call(1, 2);\\n    finish();\\n}\\n' > lib.rs && git add -A && \
         git -c user.name=t -c user.email=t@t commit -qm 'base lands a call' && \
         git push -q origin HEAD:main) && git worktree remove --force \"$tmp\"";
    let develop1 = json!([
        {"sh": base_moves},
        {"sh": format!("printf 'fn run() {{\\n    setup_mine();\\n    finish_mine();\\n}}\\n' > lib.rs && {TASK_SCRIPT_COMMIT}")},
        {"call": {"tool": "submit_change", "args": {"summary": "first try", "head": "$HEAD"}}}
    ]);
    // Merges the base, then takes its own side of the conflict: the landed line is gone.
    let develop2 = json!([
        {"sh": "printf '%s' \"$FAKE_PROMPT\" > seen-prompt.txt; \
                grep -q 'no longer merges cleanly' seen-prompt.txt || exit 1; rm seen-prompt.txt; \
                git -c user.name=t -c user.email=t@t merge -q origin/main; \
                printf 'fn run() {\\n    setup_mine();\\n    finish_mine();\\n}\\n' > lib.rs && echo ok > fixed.txt && \
                git add -A && git -c user.name=t -c user.email=t@t commit -qm 'merge the base, keep mine'"},
        {"call": {"tool": "submit_change", "args": {"summary": "merged", "head": "$HEAD"}}}
    ]);
    // Told which line was dropped, it restores it.
    let develop3 = json!([
        {"sh": "printf '%s' \"$FAKE_PROMPT\" > seen-prompt.txt; \
                grep -q 'landed_call(1, 2);' seen-prompt.txt && grep -q 'lib.rs' seen-prompt.txt || exit 1; rm seen-prompt.txt; \
                printf 'fn run() {\\n    setup_mine();\\n    landed_call(1, 2);\\n    finish_mine();\\n}\\n' > lib.rs && \
                git add -A && git -c user.name=t -c user.email=t@t commit -qm 'restore the landed call'"},
        {"call": {"tool": "submit_change", "args": {"summary": "restored", "head": "$HEAD"}}}
    ]);
    let script = json!({"steps": {"explore": [explore_turn()], "develop": [develop1, develop2, develop3]}});
    let s = Scratch::new("test -f hello.txt && test -f fixed.txt", script);
    s.push_files(
        &[("lib.rs", "fn run() {\n    setup();\n    finish();\n}\n")],
        "add lib.rs",
    );
    let d = s.start();
    let t = file_task(&d, "Rewrites a function the base changed");
    wait_state(&d, t, State::Landed, 180);

    let w = d.world().unwrap();
    assert_eq!(w.tasks[&t].failures, 0, "a dropped line is not a failure");
    assert_eq!(w.tasks[&t].attempts[&State::Developing], 3);
    // Sent back once for the dropped line, naming the file; the feedback is checked by the script.
    let reasons: Vec<String> = d
        .store
        .read_since(0, 100_000)
        .unwrap()
        .into_iter()
        .filter_map(|e| match e.event.kind {
            EventKind::SentBack { task, reason, .. } if task == t => Some(reason),
            _ => None,
        })
        .collect();
    assert!(
        reasons
            .iter()
            .any(|r| r == "conflict resolution dropped 1 landed line(s) in: lib.rs"),
        "{reasons:?}"
    );
    // What reached the remote has the base's line and the task's.
    let lib = sh(&s.origin, &["show", "main:lib.rs"]);
    assert_eq!(
        lib,
        "fn run() {\n    setup_mine();\n    landed_call(1, 2);\n    finish_mine();\n}"
    );
    assert_eq!(sh(&s.origin, &["show", "main:fixed.txt"]), "ok");
}

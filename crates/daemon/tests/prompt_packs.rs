// SPDX-License-Identifier: GPL-3.0-or-later

//! s294: a custom agent step has its own prompt file, and a develop round sent back by a red
//! test uses the `fix` pack. Both are read from the base commit; the fake agent records the
//! prompts it is given.

mod support;
use mason_core::*;
use serde_json::{json, Value};
use support::*;

const DEF: &str = r#"schema = 1
start = "develop"

[step.develop]
type = "agent"
category = "developing"
next = "test"

[step.test]
type = "test"
category = "testing"
command = "test -f hello.txt"
on_red = "develop"
max_rounds = 3
next = "security-review"

[step.security-review]
type = "agent"
category = "review"
prompt = "security-review.md"
next = "land"

[step.land]
type = "landing"
category = "landing"
"#;

fn record(dir: &std::path::Path, name: &str) -> Value {
    json!({"sh": format!("printf '%s' \"$FAKE_PROMPT\" > {}/{name}.txt", dir.display())})
}

fn commit(cmd: &str, msg: &str) -> Value {
    json!({"sh": format!("{cmd} && git -c user.name=t -c user.email=t@t commit -qm '{msg}'")})
}

fn submit() -> Value {
    json!({"call": {"tool": "submit_change", "args": {"summary": "round", "head": "$HEAD"}}})
}

fn run(files: &[(&str, &str)]) -> (String, String, String) {
    let rec = tempfile::tempdir().unwrap();
    let r = rec.path();
    let s = Scratch::new(
        "false",
        json!({"steps": {
            "develop": [
                // Round one: red (no hello.txt). Also commits an edit of the step's prompt file on the task branch.
                [record(r, "dev1"),
                 {"sh": "echo edited-by-the-task > .mason/prompts/security-review.md; echo a > a.txt; git add -A"},
                 commit("true", "first"), submit()],
                [record(r, "dev2"),
                 commit("echo hello > hello.txt && git add hello.txt", "second"), submit()],
            ],
            "review": [[record(r, "review"),
                {"call": {"tool": "submit_review", "args": {"verdict": "approve", "summary": "fine"}}}]],
        }}),
    );
    let mut all: Vec<(&str, &str)> = vec![(".mason/mason.toml", DEF)];
    all.extend_from_slice(files);
    s.push_files(&all, "project setup");
    let d = s.start();
    let t = file_task(&d, "Needs a security review");
    wait_state(&d, t, State::Landed, 600);
    let get = |n: &str| std::fs::read_to_string(r.join(format!("{n}.txt"))).unwrap_or_default();
    (get("dev1"), get("dev2"), get("review"))
}

#[test]
fn a_custom_step_and_a_fix_round_get_their_own_prompt_files() {
    let (dev1, dev2, review) = run(&[
        (
            ".mason/prompts/security-review.md",
            "SECURITY-PACK look for secrets. {task}",
        ),
        (".mason/prompts/fix.md", "FIX-PACK repair it. {feedback}"),
    ]);
    assert!(review.contains("SECURITY-PACK"), "{review}");
    assert!(
        !review.contains("edited-by-the-task"),
        "the task's own edit is not read: {review}"
    );
    assert!(!dev1.contains("FIX-PACK"), "{dev1}");
    assert!(dev2.contains("FIX-PACK"), "{dev2}");
    assert!(
        !dev2.contains("You are developing"),
        "built-in text replaced: {dev2}"
    );
}

#[test]
fn without_a_fix_pack_the_round_gets_the_built_in_develop_prompt() {
    let (_, dev2, review) = run(&[(".mason/prompts/security-review.md", "SECURITY-PACK {task}")]);
    assert!(review.contains("SECURITY-PACK"), "{review}");
    assert!(!dev2.contains("FIX-PACK"), "{dev2}");
    assert!(dev2.contains("previous attempt failed"), "{dev2}");
}

#[test]
fn a_step_whose_prompt_file_is_missing_fails_and_names_it() {
    let s = Scratch::new(
        "false",
        json!({"steps": {
            "develop": [[commit("echo hello > hello.txt && git add hello.txt", "x"), submit()]],
            "review": [[{"call": {"tool": "submit_review", "args": {"verdict": "approve", "summary": "fine"}}}]],
        }}),
    );
    s.push_files(&[(".mason/mason.toml", DEF)], "project setup");
    let d = s.start();
    let t = file_task(&d, "No pack on file");
    wait_state(&d, t, State::Failed, 600);
    let why = task_of(&d, t).failure.unwrap_or_default();
    assert!(why.contains("security-review.md"), "{why}");
}

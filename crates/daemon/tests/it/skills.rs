// SPDX-License-Identifier: GPL-3.0-or-later

//! s1153: a session's skills are the ones on the base commit. The fake agent lists the
//! skills of its `--plugin-dir` in its init event, as the real CLI does.

use crate::support;
use mason_core::*;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::Path;
use support::*;

const DEF: &str = r#"schema = 1
start = "develop"

[step.develop]
type = "agent"
category = "developing"
next = "review"

[step.review]
type = "agent"
category = "review"
next = "land"

[step.land]
type = "landing"
category = "landing"
"#;

/// The `skills` field of every init event under `dir`, as it is written.
fn init_skills(dir: &Path, found: &mut Vec<String>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = e.path();
        if path.is_dir() {
            init_skills(&path, found);
            continue;
        }
        let bytes = std::fs::read(&path).unwrap_or_default();
        let text = String::from_utf8_lossy(&bytes);
        for part in text.split("\"skills\":[").skip(1) {
            if let Some((list, _)) = part.split_once(']') {
                found.push(list.to_string());
            }
        }
    }
}

#[test]
fn a_session_has_the_base_commits_skills_and_none_the_task_adds() {
    let add = "echo hello > hello.txt && mkdir -p .mason/skills/second && \
               printf -- '---\\nname: second\\ndescription: x\\n---\\n' > .mason/skills/second/SKILL.md && \
               git add -A && git -c user.name=t -c user.email=t@t commit -qm second";
    let call: Value = json!({"tool": {"name": "Skill", "input": {"skill": "mason:fixture"}}});
    let s = Scratch::new(
        "true",
        json!({"steps": {
            "develop": [[call, {"sh": add},
                {"call": {"tool": "submit_change", "args": {"summary": "x", "head": "$HEAD"}}}]],
            "review": [[{"call": {"tool": "submit_review", "args": {"verdict": "approve", "summary": "fine"}}}]],
        }}),
    );
    s.push_files(
        &[
            (".mason/mason.toml", DEF),
            (
                ".mason/skills/fixture/SKILL.md",
                "---\nname: fixture\ndescription: A fixture.\n---\nDo the thing.\n",
            ),
        ],
        "project setup",
    );
    let d = s.start();
    let t = file_task(&d, "Adds a skill of its own");
    wait_state(&d, t, State::Landed, 600);

    let mut listed = vec![];
    init_skills(&s.data_dir, &mut listed);
    assert!(!listed.is_empty(), "no init event was found");
    let distinct: BTreeSet<&str> = listed.iter().map(String::as_str).collect();
    assert_eq!(
        distinct,
        BTreeSet::from(["\"mason:fixture\""]),
        "{listed:?}"
    );

    let w = d.world().unwrap();
    let called: Vec<(String, Vec<String>)> = w
        .step_runs
        .values()
        .filter(|r| r.task == t)
        .map(|r| {
            let skills = mason_daemon::skills::invoked(&d.cfg.task_dir(t.0), r.id.0);
            (r.name.clone(), skills)
        })
        .filter(|(_, skills)| !skills.is_empty())
        .collect();
    assert_eq!(
        called,
        [("develop".to_string(), vec!["mason:fixture".to_string()])]
    );
}

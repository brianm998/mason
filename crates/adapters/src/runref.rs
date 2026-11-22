// SPDX-License-Identifier: GPL-3.0-or-later

//! What a project's repository needs for remote runs (#703), as pure checks on text and JSON:
//! which workflows fire on a `mason/run/**` push, whether the token can push and read Actions,
//! whether secret-scanning push protection is on, and which run refs are stale.
//!
//! Workflow files are scanned as text (this crate has no YAML parser), the same way the
//! landing-CI check reads them. The scan understands the forms people write the `push` trigger
//! in: `on: push`, `on: [push, ...]`, a block with `branches:` as a flow or block list, and
//! `branches-ignore:`. It errs towards "fires": a workflow it cannot read is reported, not
//! passed.

use serde_json::Value;
use std::collections::HashSet;

/// A ref a run is pushed to, for matching filters against.
pub const SAMPLE_REF: &str = "mason/run/project/task-1";

/// How a workflow's `push` trigger filters branches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Push {
    /// The workflow has no push trigger.
    None,
    /// Pushes to any branch.
    Any,
    /// Only pushes to branches matching one of these patterns (`!` negates).
    Branches(Vec<String>),
    /// Pushes to every branch except these.
    Ignoring(Vec<String>),
    /// Only tag pushes.
    TagsOnly,
}

/// `*` matches within a path segment, `**` across segments.
pub fn glob(pat: &str, s: &str) -> bool {
    fn go(p: &[u8], s: &[u8]) -> bool {
        match p.first() {
            None => s.is_empty(),
            Some(b'*') if p.get(1) == Some(&b'*') => {
                let rest = &p[2..];
                (0..=s.len()).any(|i| go(rest, &s[i..]))
            }
            Some(b'*') => {
                let rest = &p[1..];
                let mut i = 0;
                loop {
                    if go(rest, &s[i..]) {
                        return true;
                    }
                    if i >= s.len() || s[i] == b'/' {
                        return false;
                    }
                    i += 1;
                }
            }
            Some(c) => s.first() == Some(c) && go(&p[1..], &s[1..]),
        }
    }
    go(pat.as_bytes(), s.as_bytes())
}

fn indent(l: &str) -> usize {
    l.len() - l.trim_start().len()
}

fn unquote(s: &str) -> String {
    s.trim().trim_matches(|c| c == '"' || c == '\'').to_string()
}

fn flow_items(s: &str) -> Vec<String> {
    s.trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split(',')
        .map(unquote)
        .filter(|x| !x.is_empty())
        .collect()
}

/// The items of `key:` inside `lines`: a flow list on the same line, a block list under it, or
/// one scalar.
fn list_of(lines: &[&str], key: &str) -> Option<Vec<String>> {
    let needle = format!("{key}:");
    for (i, l) in lines.iter().enumerate() {
        let t = l.trim_start();
        let at = if t.starts_with(&needle) {
            Some(0)
        } else {
            // Inside a flow mapping: `{ branches: [a], tags: [b] }`.
            t.find(&format!(" {needle}"))
                .or_else(|| t.find(&format!("{{{needle}")))
                .map(|p| p + 1)
        };
        let Some(at) = at else { continue };
        let rest = t[at + needle.len()..].trim();
        if rest.starts_with('[') {
            let mut text = rest.to_string();
            let mut j = i + 1;
            while !text.contains(']') && j < lines.len() {
                text.push_str(lines[j].trim());
                j += 1;
            }
            let end = text.find(']').unwrap_or(text.len());
            return Some(flow_items(&text[..end]));
        }
        if rest.is_empty() {
            let base = indent(l);
            let items: Vec<String> = lines[i + 1..]
                .iter()
                .take_while(|n| {
                    let nt = n.trim_start();
                    nt.is_empty() || nt.starts_with('-') || indent(n) > base
                })
                .filter_map(|n| n.trim_start().strip_prefix('-').map(unquote))
                .collect();
            return Some(items);
        }
        let rest = rest.trim_end_matches(['}', ',', ' ']);
        return Some(vec![unquote(rest)]);
    }
    None
}

/// How the workflow's `on:` reacts to a push of a branch. `create` counts as `Any`: it fires
/// when a branch is created, whatever its name.
pub fn push_trigger(text: &str) -> Push {
    let lines: Vec<&str> = text
        .lines()
        .filter(|l| !l.trim_start().starts_with('#') && !l.trim().is_empty())
        .collect();
    let Some(oi) = lines.iter().position(|l| {
        indent(l) == 0
            && ["on:", "\"on\":", "'on':", "true:"]
                .iter()
                .any(|k| l.starts_with(k))
    }) else {
        return Push::None;
    };
    let head = lines[oi].split_once(':').map_or("", |x| x.1).trim();
    let head = head.split(" #").next().unwrap_or("").trim();
    if !head.is_empty() {
        // `on: push`, `on: [push, pull_request]` or `on: { push: ... }`.
        if let Some(inner) = head.strip_prefix('{') {
            let creates = inner.contains("create:");
            let got = match inner.find("push:") {
                Some(p) => from_push_block(&[], inner[p + "push:".len()..].trim()),
                None => Push::None,
            };
            return if creates { Push::Any } else { got };
        }
        let events = flow_items(head);
        return if events.iter().any(|e| e == "push" || e == "create") {
            Push::Any
        } else {
            Push::None
        };
    }
    let block: Vec<&str> = lines[oi + 1..]
        .iter()
        .copied()
        .take_while(|l| indent(l) > 0)
        .collect();
    let child = block.iter().map(|l| indent(l)).min().unwrap_or(0);
    // The list form: `on:\n  - push`.
    let listed = block.iter().any(|l| {
        l.trim_start()
            .strip_prefix('-')
            .is_some_and(|e| matches!(unquote(e).as_str(), "push" | "create"))
    });
    if listed {
        return Push::Any;
    }
    let creates = block
        .iter()
        .any(|l| indent(l) == child && l.trim_start().starts_with("create:"));
    let Some(pi) = block
        .iter()
        .position(|l| indent(l) == child && l.trim_start().starts_with("push:"))
    else {
        return if creates { Push::Any } else { Push::None };
    };
    let inline = block[pi].trim_start()["push:".len()..].trim().to_string();
    let body: Vec<&str> = block[pi + 1..]
        .iter()
        .copied()
        .take_while(|l| indent(l) > child)
        .collect();
    let got = from_push_block(&body, &inline);
    if creates {
        Push::Any
    } else {
        got
    }
}

/// Can the workflow be started by `workflow_dispatch`? MASON starts its run workflow that way
/// (the shard list and the runner are the dispatch's inputs).
pub fn dispatches(text: &str) -> bool {
    let lines: Vec<&str> = text
        .lines()
        .filter(|l| !l.trim_start().starts_with('#') && !l.trim().is_empty())
        .collect();
    let Some(oi) = lines.iter().position(|l| {
        indent(l) == 0
            && ["on:", "\"on\":", "'on':", "true:"]
                .iter()
                .any(|k| l.starts_with(k))
    }) else {
        return false;
    };
    let head = lines[oi].split_once(':').map_or("", |x| x.1).trim();
    let head = head.split(" #").next().unwrap_or("").trim();
    if !head.is_empty() {
        return head
            .trim_matches(|c| c == '[' || c == ']' || c == '{' || c == '}')
            .split(',')
            .any(|e| unquote(e.split(':').next().unwrap_or("")) == "workflow_dispatch");
    }
    let block: Vec<&str> = lines[oi + 1..]
        .iter()
        .copied()
        .take_while(|l| indent(l) > 0)
        .collect();
    let child = block.iter().map(|l| indent(l)).min().unwrap_or(0);
    block.iter().any(|l| {
        let s = l.trim_start();
        (indent(l) == child && s.starts_with("workflow_dispatch"))
            || s.strip_prefix('-')
                .is_some_and(|e| unquote(e) == "workflow_dispatch")
    })
}

fn from_push_block(body: &[&str], inline: &str) -> Push {
    let mut all: Vec<&str> = vec![];
    if !inline.is_empty() && !inline.starts_with('#') {
        all.push(inline);
    }
    all.extend_from_slice(body);
    if let Some(b) = list_of(&all, "branches") {
        return Push::Branches(b);
    }
    if let Some(b) = list_of(&all, "branches-ignore") {
        return Push::Ignoring(b);
    }
    if list_of(&all, "tags").is_some() || list_of(&all, "tags-ignore").is_some() {
        // Tag filters alone leave branch pushes out.
        return Push::TagsOnly;
    }
    Push::Any
}

/// Does a push of `git_ref` (a branch name) start a workflow with this trigger?
pub fn fires_on(push: &Push, git_ref: &str) -> bool {
    match push {
        Push::None | Push::TagsOnly => false,
        Push::Any => true,
        Push::Branches(pats) => {
            let mut hit = false;
            for p in pats {
                match p.strip_prefix('!') {
                    Some(n) => {
                        if glob(n, git_ref) {
                            hit = false;
                        }
                    }
                    None => {
                        if glob(p, git_ref) {
                            hit = true;
                        }
                    }
                }
            }
            hit
        }
        Push::Ignoring(pats) => !pats.iter().any(|p| glob(p, git_ref)),
    }
}

/// What is wrong with the workflows of the base branch for remote runs. `files` are
/// `(file name, text)`; `workflow` is the file name `[remote_runs] workflow` names.
pub fn workflow_problems(files: &[(String, String)], workflow: &str) -> Vec<String> {
    let mut out = vec![];
    match files.iter().find(|(n, _)| n == workflow) {
        None => out.push(format!(
            "there is no .github/workflows/{workflow} on the base branch; `mason remote setup` writes it"
        )),
        Some((_, t)) => {
            if !dispatches(t) {
                out.push(format!(
                    "{workflow} does not accept `workflow_dispatch`, which is how a run starts with its shard list and runner as inputs; add `on: {{ workflow_dispatch: {{ inputs: ... }} }}` (the copy `mason remote setup` writes has it)"
                ));
            }
            if fires_on(&push_trigger(t), SAMPLE_REF) {
                out.push(format!(
                    "{workflow} also runs on pushes to `mason/run/**`, so every run would start twice (once by the push, once by the dispatch); remove its `push` trigger"
                ));
            }
        }
    }
    for (n, t) in files.iter().filter(|(n, _)| n != workflow) {
        if fires_on(&push_trigger(t), SAMPLE_REF) {
            out.push(format!(
                "{n} also runs on pushes to `mason/run/**`, so every remote run would start it too and double the runs on the shared runners; give its `push` a `branches` filter that leaves `mason/run/**` out"
            ));
        }
    }
    out
}

/// Whether secret-scanning push protection is on, from `GET /repos/{repo}`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PushProtection {
    Enabled,
    Disabled,
    /// The answer has no `security_and_analysis` (the token may not see it).
    Unknown,
}

pub fn push_protection(repo: &Value) -> PushProtection {
    match repo["security_and_analysis"]["secret_scanning_push_protection"]["status"].as_str() {
        Some("enabled") => PushProtection::Enabled,
        Some(_) => PushProtection::Disabled,
        None => PushProtection::Unknown,
    }
}

/// The repository permission that remote runs need, from `GET /repos/{repo}`. The Actions read
/// is a separate request (`actions/permissions`).
pub fn push_problem(repo: &Value) -> Option<String> {
    match repo["permissions"]["push"].as_bool() {
        Some(true) => None,
        Some(false) => Some(
            "the token cannot push to the repository, so a run's ref cannot be created".into(),
        ),
        None => Some(
            "the answer does not say whether the token can push (no `permissions`); use a token with repository access"
                .into(),
        ),
    }
}

/// Run refs to delete: older than `max_age_s` and not named by a live record. A ref whose age is
/// unknown is kept. `refs` are `(ref name, commit time in seconds)`.
pub fn stale_refs(
    refs: &[(String, Option<u64>)],
    live: &HashSet<String>,
    now_s: u64,
    max_age_s: u64,
) -> Vec<String> {
    refs.iter()
        .filter(|(r, _)| r.starts_with("mason/run/") && !live.contains(r))
        .filter(|(_, t)| t.is_some_and(|t| now_s.saturating_sub(t) > max_age_s))
        .map(|(r, _)| r.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(v: &[(&str, &str)]) -> Vec<(String, String)> {
        v.iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    }

    const RUN: &str =
        "name: r\non:\n  workflow_dispatch:\n    inputs:\n      matrix:\n        type: string\n";

    #[test]
    fn the_trigger_is_found_or_missing() {
        assert!(workflow_problems(&files(&[("mason-run.yml", RUN)]), "mason-run.yml").is_empty());
        let none = "on:\n  pull_request:\n";
        let p = workflow_problems(&files(&[("mason-run.yml", none)]), "mason-run.yml");
        assert!(
            p.iter()
                .any(|x| x.contains("does not accept `workflow_dispatch`")),
            "{p:?}"
        );
        let p = workflow_problems(&files(&[]), "mason-run.yml");
        assert!(p
            .iter()
            .any(|x| x.contains("no .github/workflows/mason-run.yml")));
        // A comment is not a trigger.
        let c = "on:\n  push:\n    branches: [develop]\n    # workflow_dispatch\n";
        assert!(!workflow_problems(&files(&[("mason-run.yml", c)]), "mason-run.yml").is_empty());
        // The old push trigger would start every run twice beside the dispatch.
        let both = "on:\n  workflow_dispatch:\n  push:\n    branches: [\"mason/run/**\"]\n";
        let p = workflow_problems(&files(&[("mason-run.yml", both)]), "mason-run.yml");
        assert!(p.iter().any(|x| x.contains("start twice")), "{p:?}");
        let old = "on:\n  push:\n    branches: [\"mason/run/**\"]\n";
        assert_eq!(
            workflow_problems(&files(&[("mason-run.yml", old)]), "mason-run.yml").len(),
            2
        );
    }

    #[test]
    fn the_dispatch_trigger_is_read_in_each_form() {
        for yes in [
            "on: workflow_dispatch\n",
            "on: [push, workflow_dispatch]\n",
            "on: { workflow_dispatch: {} }\n",
            "on:\n  - workflow_dispatch\n",
            "on:\n  workflow_dispatch:\n    inputs:\n      a:\n",
        ] {
            assert!(dispatches(yes), "{yes:?}");
        }
        for no in [
            "on: push\n",
            "on:\n  push:\n    branches: [x]\n",
            "# on: workflow_dispatch\njobs: {}\n",
            "on:\n  push:\n    # workflow_dispatch\n",
        ] {
            assert!(!dispatches(no), "{no:?}");
        }
    }

    #[test]
    fn another_workflow_with_an_unfiltered_push_is_flagged() {
        let bad = [
            "on: push\n",
            "on: [push, pull_request]\n",
            "on:\n  push:\n  pull_request:\n",
            "on:\n  push:\n    paths: [\"src/**\"]\n",
            "on:\n  push:\n    branches: ['**']\n",
            "on:\n  push:\n    branches:\n      - '**'\n",
            "on:\n  push:\n    branches-ignore: [develop]\n",
            "on:\n  create:\n",
            "on:\n  - push\n",
            "on: { push: { branches: [\"mason/**\"] } }\n",
        ];
        for b in bad {
            let p = workflow_problems(
                &files(&[("mason-run.yml", RUN), ("ci.yml", b)]),
                "mason-run.yml",
            );
            assert_eq!(p.len(), 1, "{b:?} -> {p:?}");
            assert!(p[0].starts_with("ci.yml also runs"), "{p:?}");
        }
    }

    #[test]
    fn a_filtered_or_other_event_workflow_is_not_flagged() {
        let fine = [
            "on:\n  push:\n    branches: [develop]\n",
            "on:\n  push:\n    branches: [develop, \"stage/**\"]\n    paths-ignore: [\"README.md\"]\n  pull_request:\n  schedule:\n    - cron: \"1 2 * * *\"\n",
            "on:\n  push:\n    branches:\n      - develop\n",
            "on:\n  push:\n    tags: [\"v*\"]\n  workflow_dispatch:\n",
            "on:\n  pull_request:\n    paths:\n      - \"a/**\"\n  push:\n    branches: [develop]\n",
            "on:\n  push:\n    branches-ignore: [\"mason/**\"]\n",
            "on:\n  push:\n    branches: ['**', '!mason/run/**']\n",
            "on: workflow_dispatch\n",
            "on: { push: { branches: [develop] } }\n",
        ];
        for b in fine {
            let p = workflow_problems(
                &files(&[("mason-run.yml", RUN), ("ci.yml", b)]),
                "mason-run.yml",
            );
            assert!(p.is_empty(), "{b:?} -> {p:?}");
        }
    }

    #[test]
    fn push_protection_reads_the_settings() {
        let on = serde_json::json!({"security_and_analysis": {"secret_scanning_push_protection": {"status": "enabled"}}});
        let off = serde_json::json!({"security_and_analysis": {"secret_scanning_push_protection": {"status": "disabled"}}});
        assert_eq!(push_protection(&on), PushProtection::Enabled);
        assert_eq!(push_protection(&off), PushProtection::Disabled);
        assert_eq!(
            push_protection(&serde_json::json!({})),
            PushProtection::Unknown
        );
    }

    #[test]
    fn the_push_permission_is_read() {
        assert!(push_problem(&serde_json::json!({"permissions": {"push": true}})).is_none());
        assert!(push_problem(&serde_json::json!({"permissions": {"push": false}})).is_some());
        assert!(push_problem(&serde_json::json!({})).is_some());
    }

    #[test]
    fn only_old_refs_without_a_live_record_are_stale() {
        let day = 24 * 3600;
        let now = 10 * day;
        let refs = vec![
            ("mason/run/p/old".to_string(), Some(now - day - 1)),
            ("mason/run/p/new".to_string(), Some(now - day + 1)),
            ("mason/run/p/live".to_string(), Some(now - 5 * day)),
            ("mason/run/p/unknown".to_string(), None),
            ("mason/ci/other".to_string(), Some(0)),
        ];
        let live: HashSet<String> = ["mason/run/p/live".to_string()].into();
        assert_eq!(stale_refs(&refs, &live, now, day), vec!["mason/run/p/old"]);
    }
}

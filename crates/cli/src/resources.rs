// SPDX-License-Identifier: GPL-3.0-or-later

//! `mason run --needs`, `mason resources`, and the optional agent hook.
//!
//! An interactive caller queues for the same resources as the pipeline, ahead
//! of pipeline work. The daemon decides when it is its turn; the caller then
//! takes the kernel locks itself, so they vanish if it is killed and the next
//! waiter starts at once.

use crate::client::{CallError, Daemon};
use mason_resources::{Locks, Take, TakeError};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

/// `compile`, `compile=2,test-suite` -> counts.
pub fn parse_needs(spec: &str) -> Result<BTreeMap<String, u32>, String> {
    let mut out = BTreeMap::new();
    for part in spec.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let (name, n) = match part.split_once('=') {
            Some((n, c)) => (
                n.trim(),
                c.trim()
                    .parse::<u32>()
                    .map_err(|_| format!("`{part}`: the count must be a whole number"))?,
            ),
            None => (part, 1),
        };
        if name.is_empty() || n == 0 {
            return Err(format!("`{part}` is not a resource and a count"));
        }
        *out.entry(name.to_string()).or_insert(0) += n;
    }
    if out.is_empty() {
        return Err("--needs wants at least one resource, like `compile`".into());
    }
    Ok(out)
}

fn say(e: CallError) -> String {
    e.to_string()
}

/// Run `cmd` once the resources are ours. Returns the command's exit code.
pub fn run(
    d: &Daemon,
    uses: BTreeMap<String, u32>,
    label: Option<String>,
    cmd: &[String],
) -> Result<i32, String> {
    // A run made by a task's agent (MASON_TASK is in the step environment) is
    // that task's pipeline work; without it, it is a person's.
    let task: Option<u64> = std::env::var("MASON_TASK")
        .ok()
        .and_then(|v| v.trim().parse().ok());
    let label = label.unwrap_or_else(|| match task {
        Some(t) => format!("task {t} `{}`", cmd.join(" ")),
        None => format!("interactive `{}`", cmd.join(" ")),
    });
    let opened = d
        .call(
            "POST",
            "/leases",
            Some(&json!({"uses": uses, "label": label, "task": task})),
        )
        .map_err(say)?;
    let id = opened["id"].as_u64().ok_or("the daemon sent no lease id")?;
    let release = |d: &Daemon| {
        let _ = d.call("POST", &format!("/leases/{id}/release"), Some(&json!({})));
    };
    let mut last = String::new();
    let granted = loop {
        let s = d.call("GET", &format!("/leases/{id}"), None).map_err(say)?;
        match s["state"].as_str() {
            Some("waiting") => {
                let why = s["explain"].as_str().unwrap_or("waiting").to_string();
                if why != last {
                    eprintln!("mason: {why}");
                    last = why;
                }
                std::thread::sleep(Duration::from_millis(150));
            }
            Some("granted") => break s,
            _ => return Err("the daemon dropped the request".into()),
        }
    };
    // Take the locks ourselves: they die with us.
    let held = {
        let locks = Locks::new(PathBuf::from(
            granted["locks_dir"].as_str().unwrap_or_default(),
        ))
        .map_err(|e| e.to_string())?;
        let take = Take {
            classes: granted["classes"]
                .as_object()
                .into_iter()
                .flatten()
                .map(|(k, v)| (k.clone(), v.as_u64().unwrap_or(0) as u32))
                .collect(),
            exclusive: granted["exclusive"].as_bool().unwrap_or(false),
        };
        let caps: BTreeMap<String, u32> = granted["capacity"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(k, v)| (k.clone(), v.as_u64().unwrap_or(0) as u32))
            .collect();
        let since = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |t| t.as_millis() as u64);
        let started = Instant::now();
        loop {
            match locks.take(id, &label, since, &take, &caps) {
                Ok(h) => break h,
                Err(TakeError::Busy) if started.elapsed() < Duration::from_secs(30) => {
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(e) => {
                    release(d);
                    return Err(format!("could not take the granted resources: {e}"));
                }
            }
        }
    };
    let status = Command::new(&cmd[0]).args(&cmd[1..]).status();
    drop(held);
    release(d);
    match status {
        Ok(s) => Ok(s.code().unwrap_or(128 + signal_of(&s))),
        Err(e) => Err(format!("cannot run `{}`: {e}", cmd[0])),
    }
}

fn signal_of(s: &std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    s.signal().unwrap_or(1)
}

fn ago(now: u64, since: u64) -> String {
    let s = now.saturating_sub(since) / 1000;
    match s {
        0..=59 => format!("{s} s"),
        60..=3599 => format!("{} min", s / 60),
        _ => format!("{} h {} min", s / 3600, (s % 3600) / 60),
    }
}

pub fn show(d: &Daemon, json_out: bool) -> Result<(), String> {
    let v = d.call("GET", "/resources", None).map_err(say)?;
    if json_out {
        println!("{v:#}");
        return Ok(());
    }
    let now = v["now_ms"].as_u64().unwrap_or(0);
    println!("Resources");
    for r in v["resources"].as_array().into_iter().flatten() {
        println!(
            "  {:<18} {:<8} {}/{} in use",
            r["name"].as_str().unwrap_or("?"),
            r["kind"].as_str().unwrap_or("?"),
            r["used"],
            r["limit"]
        );
        for (i, s) in r["slots"].as_array().into_iter().flatten().enumerate() {
            if let Some(by) = s["held_by"].as_str() {
                println!(
                    "      slot {i}: held by {by} for {}",
                    ago(now, s["since_ms"].as_u64().unwrap_or(0))
                );
            }
        }
    }
    if let Some(max) = v["cpu"]["compile_max_pct"].as_u64() {
        match v["cpu"]["busy_pct"].as_u64() {
            Some(b) => println!("  cpu {b}% busy, a compile starts below {max}%"),
            None => println!("  cpu not readable, a compile starts below {max}% (no gate now)"),
        }
    }
    if let Some(f) = v["memory"]["floor_mb"].as_u64() {
        println!("  memory floor {f} MB ({} MB free)", v["memory"]["free_mb"]);
    }
    let waiting = v["waiting"].as_array().cloned().unwrap_or_default();
    if waiting.is_empty() {
        println!("Nothing is waiting.");
    } else {
        println!("Waiting, in line order");
        for w in waiting {
            println!(
                "  {} {}",
                w["label"].as_str().unwrap_or("?"),
                w["explain"].as_str().unwrap_or("")
            );
        }
    }
    Ok(())
}

pub fn limit(d: &Daemon, name: &str, units: u32) -> Result<(), String> {
    d.call(
        "POST",
        "/resources/limit",
        Some(&json!({"resource": name, "units": units})),
    )
    .map_err(say)?;
    println!("{name} limit is now {units}. Anything running keeps running; new work waits until the count falls.");
    Ok(())
}

/// Commands the optional agent hook routes through the queue.
const WRAPPED: &[&str] = &[
    "cargo",
    "xcodebuild",
    "swift",
    "gradle",
    "./gradlew",
    "make",
    "cmake",
    "ninja",
    "pytest",
];

/// The optional Claude Code `PreToolUse` hook for Bash: reads the hook input on
/// stdin and, for a build or test command, answers with the same command run
/// under `mason run --needs compile`. It asks rather than allows, so the owner
/// still sees what will run. Anything else is left alone (no output).
pub fn hook_bash(input: &str, needs: &str, mason: &str) -> Option<Value> {
    let v: Value = serde_json::from_str(input).ok()?;
    if v["tool_name"].as_str() != Some("Bash") {
        return None;
    }
    let command = v["tool_input"]["command"].as_str()?;
    if command.trim_start().starts_with("mason ") || !runs_wrapped(command) {
        return None;
    }
    let mut input = v["tool_input"].clone();
    input["command"] = json!(format!(
        "{} run --needs {needs} -- bash -c {}",
        if mason
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-".contains(c))
        {
            mason.to_string()
        } else {
            shell_quote(mason)
        },
        shell_quote(command)
    ));
    Some(json!({"hookSpecificOutput": {
        "hookEventName": "PreToolUse",
        "permissionDecision": "ask",
        "permissionDecisionReason": "MASON: run through the shared build queue",
        "updatedInput": input,
    }}))
}

/// Whether any part of a compound command (`cd x && cargo test`, `a; b`,
/// `FOO=1 cargo build`) starts with a wrapped tool. Looking only at the first
/// word let `cd dir && cargo test --workspace` straight past the queue.
fn runs_wrapped(command: &str) -> bool {
    command
        .split(['&', '|', ';', '\n', '(', ')', '{', '}'])
        .any(|part| {
            part.split_whitespace()
                .find(|w| !w.contains('=') || w.starts_with('='))
                .is_some_and(|w| WRAPPED.contains(&w))
        })
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needs_parse() {
        let n = parse_needs("compile, test-suite=2,compile").unwrap();
        assert_eq!(n["compile"], 2);
        assert_eq!(n["test-suite"], 2);
        for bad in ["", ",", "compile=0", "compile=x", "=2"] {
            assert!(parse_needs(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn the_hook_wraps_builds_and_leaves_everything_else() {
        let input =
            |c: &str| json!({"tool_name": "Bash", "tool_input": {"command": c}}).to_string();
        let out = hook_bash(&input("cargo test -p x"), "compile", "mason").unwrap();
        let cmd = out["hookSpecificOutput"]["updatedInput"]["command"]
            .as_str()
            .unwrap();
        assert_eq!(
            cmd,
            "mason run --needs compile -- bash -c 'cargo test -p x'"
        );
        assert_eq!(out["hookSpecificOutput"]["permissionDecision"], "ask");
        let quoted = hook_bash(&input("make it's"), "compile", "mason").unwrap();
        assert!(quoted["hookSpecificOutput"]["updatedInput"]["command"]
            .as_str()
            .unwrap()
            .ends_with("'make it'\\''s'"));
        assert!(hook_bash(&input("ls"), "compile", "mason").is_none());
        // Anywhere in a compound command, and by the CLI's own path.
        for c in [
            "cd crates && cargo test --workspace",
            "FOO=1 cargo test",
            "ls; cargo build",
            "(cd x && cargo test)",
            "true | make",
        ] {
            assert!(hook_bash(&input(c), "compile", "mason").is_some(), "{c}");
        }
        let abs = hook_bash(&input("cargo test"), "compile", "/a b/mason").unwrap();
        assert!(abs["hookSpecificOutput"]["updatedInput"]["command"]
            .as_str()
            .unwrap()
            .starts_with("'/a b/mason' run --needs compile -- bash -c"));
        assert!(hook_bash(&input("echo cargo"), "compile", "mason").is_none());
        assert!(hook_bash(
            &input("mason run --needs compile -- cargo build"),
            "compile",
            "mason"
        )
        .is_none());
        assert!(hook_bash(
            &json!({"tool_name": "Read", "tool_input": {}}).to_string(),
            "compile",
            "mason"
        )
        .is_none());
        assert!(hook_bash("not json", "compile", "mason").is_none());
    }
}

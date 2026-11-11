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

/// The CPU line of `mason resources`: the reading, where it came from, and
/// how many reads have failed.
pub fn cpu_line(cpu: &Value, max: u64) -> String {
    let source = cpu["source"].as_str();
    let failed = cpu["failed_reads"].as_u64().unwrap_or(0);
    let tail = match source {
        Some(s) if failed > 0 => format!(" [source {s}, {failed} failed reads]"),
        Some(s) => format!(" [source {s}]"),
        None => String::new(),
    };
    match (cpu["busy_pct"].as_u64(), source) {
        (Some(b), Some("loadavg")) => format!(
            "cpu {b}% busy (load average; the CPU reading is missing), a compile starts below {max}%{tail}"
        ),
        (Some(b), Some("assumed-busy")) => format!(
            "cpu assumed {b}% busy (nothing is readable, so new compiles wait), a compile starts below {max}%{tail}"
        ),
        (Some(b), _) => format!("cpu {b}% busy, a compile starts below {max}%{tail}"),
        (None, _) => format!(
            "cpu unknown (this daemon reports no reading), a compile starts below {max}% (no gate now)"
        ),
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
        println!("  {}", cpu_line(&v["cpu"], max));
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

/// `cargo` subcommands that compile nothing, so need no slot.
const CARGO_NO_COMPILE: &[&str] = &[
    "fmt",
    "metadata",
    "tree",
    "clean",
    "version",
    "--version",
    "-V",
    "locate-project",
    "pkgid",
    "search",
    "help",
];

/// Whether any part of a compound command (`cd x && cargo test`, `a; b`,
/// `FOO=1 cargo build`) starts with a wrapped tool. Looking only at the first
/// word let `cd dir && cargo test --workspace` straight past the queue. Quoted
/// text is not split: `grep "a|cargo b"` runs grep, not cargo, and a slot taken
/// for it is a slot every waiting task goes without.
fn runs_wrapped(command: &str) -> bool {
    split_unquoted(command).iter().any(|part| {
        let mut words = part
            .split_whitespace()
            .skip_while(|w| w.contains('=') && !w.starts_with('='));
        match words.next() {
            Some("cargo") => words
                .find(|w| !w.starts_with('+'))
                .is_none_or(|sub| !CARGO_NO_COMPILE.contains(&sub)),
            Some(w) => WRAPPED.contains(&w),
            None => false,
        }
    })
}

/// The parts of a command line between `& | ; ( ) { }` and newlines, leaving
/// whatever is inside quotes (or escaped) in the word it belongs to.
fn split_unquoted(command: &str) -> Vec<String> {
    let (mut parts, mut cur) = (vec![], String::new());
    let (mut single, mut double, mut escaped) = (false, false, false);
    for c in command.chars() {
        if escaped {
            escaped = false;
            cur.push(c);
            continue;
        }
        match c {
            '\\' if !single => {
                escaped = true;
                cur.push(c);
            }
            '\'' if !double => {
                single = !single;
                cur.push(c);
            }
            '"' if !single => {
                double = !double;
                cur.push(c);
            }
            '&' | '|' | ';' | '\n' | '(' | ')' | '{' | '}' if !single && !double => {
                parts.push(std::mem::take(&mut cur));
            }
            _ => cur.push(c),
        }
    }
    parts.push(cur);
    parts
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cpu_line_names_the_source_and_never_says_no_gate_for_a_fallback() {
        let l = |v: Value| cpu_line(&v, 50);
        let mach = l(json!({"busy_pct": 20, "source": "mach", "failed_reads": 0}));
        assert_eq!(
            mach,
            "cpu 20% busy, a compile starts below 50% [source mach]"
        );
        let la = l(json!({"busy_pct": 100, "source": "loadavg", "failed_reads": 3}));
        assert!(la.contains("load average") && la.contains("source loadavg, 3 failed reads"));
        assert!(!la.contains("no gate"));
        let ab = l(json!({"busy_pct": 100, "source": "assumed-busy", "failed_reads": 3}));
        assert!(ab.contains("assumed 100% busy") && ab.contains("new compiles wait"));
        assert!(!ab.contains("no gate"));
    }

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
        // Quoted text is not a command, and some cargo commands compile nothing.
        for c in [
            r#"ps ax | grep -E "step-host|cargo |make""#,
            "grep 'a;cargo test' notes.txt",
            "cargo fmt --all --check",
            "cargo +nightly fmt",
            "cd x && cargo metadata --format-version 1",
            "cargo --version",
        ] {
            assert!(hook_bash(&input(c), "compile", "mason").is_none(), "{c}");
        }
        for c in [
            "cargo fmt --check && cargo clippy",
            "cargo",
            r#"echo "done" ; cargo build"#,
        ] {
            assert!(hook_bash(&input(c), "compile", "mason").is_some(), "{c}");
        }
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

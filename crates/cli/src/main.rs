// SPDX-License-Identifier: GPL-3.0-or-later

//! `mason`: the command-line client.
//!
//! ```text
//! mason task add <title> [--project P] [--body TEXT | --body-file F | --body -] [--priority urgent|high|normal|low] [--proposed]
//! mason task list [--state S] [--json]
//! mason task show <id> [--json]
//! mason task cancel <id> [--delete-branch]
//! mason task retry <id> [--from exploring|developing|testing|review|landing]
//! mason logs <id> [--follow]
//! mason status [--json]
//! mason doctor
//! ```
//! Global: `--data-dir D`, `--config-dir C` (or `MASON_DATA_DIR`, `MASON_CONFIG_DIR`).

mod client;
mod doctor;

use client::{CallError, Daemon};
use serde_json::{json, Value};
use std::io::Read;
use std::path::PathBuf;
use std::process::exit;
use std::time::Duration;

const HELP: &str = "\
mason: command-line client for the MASON pipeline

  mason task add <title> [--project P] [--body TEXT | --body-file F | --body -]
                         [--priority urgent|high|normal|low] [--proposed]
  mason task list [--state S] [--json]
  mason task show <id> [--json]
  mason task cancel <id> [--delete-branch]
  mason task retry <id> [--from exploring|developing|testing|review|landing]
  mason logs <id> [--follow]
  mason status [--json]
  mason doctor

Global options: --data-dir D, --config-dir C (or MASON_DATA_DIR, MASON_CONFIG_DIR)";

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

struct Args {
    rest: Vec<String>,
}

impl Args {
    fn flag(&mut self, name: &str) -> bool {
        match self.rest.iter().position(|a| a == name) {
            Some(i) => {
                self.rest.remove(i);
                true
            }
            None => false,
        }
    }

    fn opt(&mut self, name: &str) -> Option<String> {
        let i = self.rest.iter().position(|a| a == name)?;
        self.rest.remove(i);
        if i < self.rest.len() {
            Some(self.rest.remove(i))
        } else {
            die(&format!("{name} needs a value"))
        }
    }

    fn pos(&mut self) -> Option<String> {
        if self.rest.is_empty() {
            None
        } else {
            Some(self.rest.remove(0))
        }
    }
}

fn die(msg: &str) -> ! {
    eprintln!("error: {msg}");
    exit(1)
}

fn or_die<T>(r: Result<T, CallError>) -> T {
    r.unwrap_or_else(|e| die(&e.to_string()))
}

fn task_id(a: &mut Args) -> u64 {
    let s = a
        .pos()
        .unwrap_or_else(|| die("which task? give its number"));
    s.trim_start_matches('#')
        .parse()
        .unwrap_or_else(|_| die(&format!("{s:?} is not a task number")))
}

fn finish(a: &Args) {
    if let Some(extra) = a.rest.first() {
        die(&format!("unexpected argument {extra:?}\n\n{HELP}"));
    }
}

fn row(v: &Value) -> String {
    format!(
        "#{:<4} {:<15} {:<10} {}\n      {}",
        v["id"],
        v["state"].as_str().unwrap_or("?"),
        v["project"].as_str().unwrap_or("?"),
        v["title"].as_str().unwrap_or(""),
        v["activity"].as_str().unwrap_or("")
    )
}

fn ago(ms: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    let s = now.saturating_sub(ms) / 1000;
    match s {
        0..=59 => format!("{s}s ago"),
        60..=3599 => format!("{}m ago", s / 60),
        _ => format!("{}h ago", s / 3600),
    }
}

fn main() {
    let mut a = Args {
        rest: std::env::args().skip(1).collect(),
    };
    if a.rest.is_empty() || a.flag("--help") || a.flag("-h") || a.rest[0] == "help" {
        println!("{HELP}");
        return;
    }
    let default_data = if cfg!(target_os = "macos") {
        home().join("Library/Application Support/mason")
    } else {
        home().join(".local/share/mason")
    };
    let data_dir = a
        .opt("--data-dir")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("MASON_DATA_DIR").map(PathBuf::from))
        .unwrap_or(default_data);
    let config_dir = a
        .opt("--config-dir")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("MASON_CONFIG_DIR").map(PathBuf::from))
        .unwrap_or_else(|| home().join(".config/mason"));
    let json_out = a.flag("--json");
    let cmd = a.pos().unwrap_or_default();

    if cmd == "doctor" {
        finish(&a);
        let findings = doctor::run(&data_dir, &config_dir);
        let mut bad = false;
        for f in &findings {
            let mark = match f.level {
                doctor::Level::Ok => "ok  ",
                doctor::Level::Warn => "warn",
                doctor::Level::Fail => {
                    bad = true;
                    "FAIL"
                }
            };
            println!("{mark}  {}", f.text);
        }
        exit(i32::from(bad));
    }

    let d: Daemon = or_die(client::find(&data_dir));
    match cmd.as_str() {
        "status" => {
            finish(&a);
            let s = or_die(d.call("GET", "/status", None));
            if json_out {
                return println!("{s:#}");
            }
            println!(
                "masond pid {}, up {}s, event log at seq {}",
                s["pid"], s["uptime_s"], s["seq"]
            );
            let counts: Vec<String> = s["tasks"]
                .as_object()
                .into_iter()
                .flatten()
                .filter(|(_, n)| n.as_u64().unwrap_or(0) > 0)
                .map(|(k, n)| format!("{n} {k}"))
                .collect();
            println!(
                "tasks: {}",
                if counts.is_empty() {
                    "none".into()
                } else {
                    counts.join(", ")
                }
            );
            println!(
                "running: {} job(s) of {} allowed, {} step host(s)",
                s["jobs"], s["max_concurrent"], s["step_hosts_running"]
            );
            println!(
                "projects: {}",
                s["projects"].as_array().map_or(String::new(), |p| p
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", "))
            );
            let stalls = s["event_loop_stalls"].as_u64().unwrap_or(0);
            if stalls > 0 {
                println!(
                    "event loop stalls: {stalls} (worst {} ms)",
                    s["worst_stall_ms"]
                );
            }
            if s["bugs"].as_u64().unwrap_or(0) > 0 {
                println!("bugs filed by the reconcile sweep: {}", s["bugs"]);
            }
        }
        "logs" => {
            let id = task_id(&mut a);
            let follow = a.flag("--follow") || a.flag("-f");
            finish(&a);
            let mut offset = 0u64;
            loop {
                let r = or_die(d.call("GET", &format!("/tasks/{id}/logs?offset={offset}"), None));
                print!("{}", r["text"].as_str().unwrap_or_default());
                offset = r["next"].as_u64().unwrap_or(offset);
                if !follow {
                    break;
                }
                let t = or_die(d.call("GET", &format!("/tasks/{id}"), None));
                if matches!(t["state"].as_str(), Some("landed" | "failed" | "canceled")) {
                    let r =
                        or_die(d.call("GET", &format!("/tasks/{id}/logs?offset={offset}"), None));
                    print!("{}", r["text"].as_str().unwrap_or_default());
                    break;
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        }
        "task" => {
            let sub = a
                .pos()
                .unwrap_or_else(|| die("task what? add, list, show, cancel or retry"));
            match sub.as_str() {
                "add" => {
                    let body = if let Some(b) = a.opt("--body") {
                        if b == "-" {
                            let mut s = String::new();
                            let _ = std::io::stdin().read_to_string(&mut s);
                            s
                        } else {
                            b
                        }
                    } else if let Some(f) = a.opt("--body-file") {
                        std::fs::read_to_string(&f)
                            .unwrap_or_else(|e| die(&format!("cannot read {f}: {e}")))
                    } else {
                        String::new()
                    };
                    let project = a.opt("--project");
                    let priority = a.opt("--priority").unwrap_or_else(|| "normal".into());
                    let proposed = a.flag("--proposed");
                    let title = a.pos().unwrap_or_else(|| {
                        die("give the task a title: mason task add \"Fix the thing\"")
                    });
                    finish(&a);
                    let project = project.unwrap_or_else(|| {
                        let s = or_die(d.call("GET", "/status", None));
                        let ps: Vec<String> = s["projects"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(Value::as_str)
                            .map(String::from)
                            .collect();
                        match ps.as_slice() {
                            [one] => one.clone(),
                            [] => die("no projects are configured (see `mason doctor`)"),
                            many => die(&format!(
                                "which project? use --project with one of: {}",
                                many.join(", ")
                            )),
                        }
                    });
                    let r = or_die(d.call("POST", "/tasks", Some(&json!({"project": project, "title": title, "body": body, "priority": priority, "proposed": proposed}))));
                    if json_out {
                        return println!("{r:#}");
                    }
                    println!("Filed task #{} in {project}", r["task"]);
                }
                "list" => {
                    let state = a.opt("--state");
                    finish(&a);
                    let path = state.map_or("/tasks".to_string(), |s| format!("/tasks?state={s}"));
                    let r = or_die(d.call("GET", &path, None));
                    if json_out {
                        return println!("{r:#}");
                    }
                    let tasks = r["tasks"].as_array().cloned().unwrap_or_default();
                    if tasks.is_empty() {
                        println!("no tasks");
                    }
                    for t in tasks {
                        println!("{}", row(&t));
                    }
                }
                "show" => {
                    let id = task_id(&mut a);
                    finish(&a);
                    let t = or_die(d.call("GET", &format!("/tasks/{id}"), None));
                    if json_out {
                        return println!("{t:#}");
                    }
                    println!("#{} {}", t["id"], t["title"].as_str().unwrap_or(""));
                    println!("  project   {}", t["project"].as_str().unwrap_or("?"));
                    println!("  state     {}", t["state"].as_str().unwrap_or("?"));
                    println!("  activity  {}", t["activity"].as_str().unwrap_or("?"));
                    if let Some(l) = t["landed"].as_str() {
                        println!("  landed    {l}");
                    }
                    println!(
                        "  filed     {}",
                        ago(t["enqueued_at"].as_u64().unwrap_or(0))
                    );
                    println!("  branch    {}", t["branch"].as_str().unwrap_or(""));
                    println!("  worktree  {}", t["worktree"].as_str().unwrap_or(""));
                    println!("  failures  {}", t["failures"]);
                    if let Some(q) = t["question"].as_str() {
                        println!("  question  {q}");
                    }
                    if let Some(b) = t["body"].as_str().filter(|b| !b.trim().is_empty()) {
                        println!("\n{}", b.trim());
                    }
                    println!("\nsteps:");
                    for s in t["steps"].as_array().into_iter().flatten() {
                        println!(
                            "  run {:<3} {:<11} attempt {}  {}",
                            s["run"],
                            s["step"].as_str().unwrap_or("?"),
                            s["attempt"],
                            s["outcome"].as_str().unwrap_or("running")
                        );
                    }
                }
                "cancel" => {
                    let id = task_id(&mut a);
                    let del = a.flag("--delete-branch");
                    finish(&a);
                    or_die(d.call(
                        "POST",
                        &format!("/tasks/{id}/cancel"),
                        Some(&json!({"delete_branch": del})),
                    ));
                    println!(
                        "Cancelled task #{id}{}",
                        if del {
                            " (branch deleted)"
                        } else {
                            " (its branch is kept)"
                        }
                    );
                }
                "retry" => {
                    let id = task_id(&mut a);
                    let from = a.opt("--from").unwrap_or_else(|| "developing".into());
                    finish(&a);
                    let r = or_die(d.call(
                        "POST",
                        &format!("/tasks/{id}/retry"),
                        Some(&json!({"to": from})),
                    ));
                    println!(
                        "Retrying task #{id} from {}",
                        r["to"].as_str().unwrap_or(&from)
                    );
                }
                other => die(&format!("unknown task command {other:?}\n\n{HELP}")),
            }
        }
        other => die(&format!("unknown command {other:?}\n\n{HELP}")),
    }
}

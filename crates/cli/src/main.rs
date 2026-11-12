// SPDX-License-Identifier: GPL-3.0-or-later

//! `mason`: the command-line client.
//!
//! ```text
//! mason task add <title> [--project P] [--body TEXT | --body-file F | --body -] [--priority urgent|high|normal|low] [--proposed]
//! mason task list [--state S] [--json]
//! mason task show <id> [--json] [--watch]
//! mason task cancel <id> [--delete-branch]
//! mason task retry <id> [--from exploring|developing|testing|review|landing]
//! mason logs <id> [--follow]
//! mason task skip <id> (--suite S | --step S) [--reason R] [--deferred]
//! mason debt list | run <id>
//! mason quarantine list | add <suite> <test> --owner O --days N [--reason R] | remove <suite> <test>
//! mason test parse <parser> <file> [--min N]
//! mason task answer <id> <option number | text> [--card HASH] [--as NAME]
//! mason task land-now <id> | force <id> [--reason R]
//! mason task open <id> [--anyway]
//! mason pause [--project P] [--mode all|claims|landing] --for 2h | mason resume [--project P] [--mode M] | mason pause list
//! mason alerts [--all] | mason metrics [--days N]
//! mason health | mason digest [--days N] [--project P] | mason schedule list | run <name>
//! mason triage list | review <id> (--agree | --disagree)
//! mason status [--json] [--watch]
//! mason run --needs compile[=N],... [--exclusive] [--label L] -- <cmd>...
//! mason hold --needs R[,R...] [--exclusive] --pipeline NAME --label L [--pid P]
//! mason targets list [--json]
//! mason resources [--json] | mason resources limit <resource> <units>
//! mason doctor
//! mason init [--yes] [--tracker T] [--agent A] [--sessions N] [--workflow W] [--suites accept|skip] [--base B] [--landing L] [--alerts LIST|none] [--no-prove]
//! mason adopt | mason remove [--yes]
//! ```
//! Global: `--data-dir D`, `--config-dir C` (or `MASON_DATA_DIR`, `MASON_CONFIG_DIR`).

mod adopt;
mod client;
mod definition;
mod doctor;
mod init;
mod progress;
mod remove;
mod resources;
mod testing;

use client::{CallError, Daemon};
use serde_json::{json, Value};
use std::io::Read;
use std::path::PathBuf;
use std::process::exit;
use std::time::Duration;

const HELP: &str = "\
mason: command-line client for the MASON pipeline

  mason task add <title> [--project P] [--body TEXT | --body-file F | --body -]
                         [--priority urgent|high|normal|low] [--proposed] [--explored]
                         [--after ID[,ID...]]   wait for those tasks to land first
  mason task list [--state S] [--json]
  mason task list --pending-issues    open issues the tracker has not imported, with why
  mason task relink <id> --issue N    tie a live task to issue N whose task was canceled (owner only)
  mason task unblock <id> [--from ID[,ID...] | --all]   release a queued task from some or all of its blockers (owner only)
  mason task block <id> --after ID[,ID...]   add blockers to a task that has not started (owner only)
  mason task revive <id>              put a canceled task whose branch was kept back in the queue (owner only)
  mason task show <id> [--json] [--watch]   with --watch, follow the live step progress
  mason task cancel <id> [--delete-branch]
  mason task retry <id> [--from STEP]
  mason task approve <id>             let a proposed task start, or an approval step go on
  mason task reject <id> [--reason R] reject a proposed task or an approval step
  mason task explore-anyway <id>      explore after all, even if it skipped exploring
  mason task override <id> <key> <value>
  mason task answer <id> <option number | text> [--card HASH] [--as NAME]
                                      answer the open question; the card's hash guards against a stale one
  mason task refresh <id>             ask the tracker now (what opening a task does)
  mason task land-now <id> | force <id> [--reason R]   marks the task and records an override and a debt
  mason task open <id> [--anyway]     open an interactive agent in the task's worktree, resuming its session
  mason pause [--project P] [--mode all|claims|landing] --for 2h   hold work back (all stops at the next step)
  mason resume [--project P] [--mode M] | mason pause list
  mason account hold <name> [--for 2h] [--reason R] | mason account release <name>
                                      hold an LLM account by hand, or release a hold now (try again at once)
  mason alerts [--all]                what needs a look right now (and, with --all, what already cleared)
  mason alerts ack KEY                acknowledge an alert: it stops reminding and stays listed while true
  mason notify [status]               where each condition is routed, and what is set up (notify.toml)
  mason notify test CHANNEL           send a test through sound, speech, push, email or webhook
  mason metrics [--days N]            interventions per landed task
  mason health                        the detectors' findings, verdicts on fixes, recurring jobs, triage review queue
  mason digest [--days N] [--project P]   landed, interventions, time by state, skips, forced landings (recorded data only)
  mason schedule list | run <name>    recurring jobs (introspect, analyst:<project>, <project>/<schedule>); run one now
  mason triage list | review <id> (--agree | --disagree)   the triage agent's sampled suggestions; it never answers
  mason task skip <id> (--suite S | --step S) [--reason R] [--deferred]
                                      skip a suite or step by force, even mid-run; records a debt
  mason debt list | run <id>          what was skipped and not yet proven; run one on the base branch
  mason quarantine list | add <suite> <test> --owner O --days N [--reason R] | remove <suite> <test>
  mason test parse <parser> <file> [--min N]   read a results file the way a suite would, and judge it
  mason logs <id> [--follow]
  mason status [--json] [--watch]     with --watch, follow every running step's progress
  mason doctor                        checks the setup, and that `mason` on PATH is this program
  mason init [--yes] [--workflow solo-fast|reviewed-pr|batched-ci|device-mobile|human-gated]
             [--tracker local|github] [--agent claude|codex] [--agent-bin P] [--sessions N]
             [--suites accept|skip] [--base B] [--landing direct|direct-ci|pr|batched]
             [--alerts sound,speech,email,push|none] [--ci-workflow yes|no] [--id ID]
             [--no-prove] [--force]
                                      guided setup: writes .mason/ and the Filing notes on branch
                                      mason/setup (no push), and the project file; every question has a flag
  mason init --prove-only             file the one harmless first task, once masond knows the project
  mason adopt [--id ID] [--base B] [--test CMD]   register a project that already has .mason/ (never writes it)
  mason remove [--yes] [--id ID]      delete the project file, .mason/ and the Filing notes (lists first without --yes)
  mason run --needs compile[=N],... [--exclusive] [--label L] -- <cmd>...
                                      queue for the pipeline's resources, ahead of its work
  mason run ... --pipeline NAME [--priority urgent|high|normal|low] [--progress 0-6]
                                      the same for another pipeline's work: ranked with MASON's tasks, behind a person's
  mason hold --needs R[,R...] [--exclusive] --pipeline NAME --label L [--pid P]
                                      hold resources for a process's life: prints `granted`, then holds until
                                      stdin closes, --pid exits or it is signalled
  mason resources [--json]            limits, who holds what, and every wait explained
  mason targets list [--json]         simulators, emulators and phones, and who has each
  mason resources limit <resource> <units>   change a limit while things run (llm:claude, compile, ...)
  mason hook bash [--needs compile]   optional Claude Code PreToolUse hook: builds go through the queue
  mason definition check [FILE]       validate a definition (default .mason/mason.toml) and dry-run it
  mason definition dry-run [FILE] [--scenarios FILE]
  mason editor show|op|undo|redo|discard|simulate|apply [--project P]   the pipeline editor, without a screen
  mason definition show [--project P] | reload [--project P]
  mason extension list|show|approve [NAME] [--project P]   project extensions; approving is the owner's
  mason lan status                    whether LAN access is on, its port and certificate fingerprint
  mason pair [--viewer]               a one-time code (and mason://pair URL) for a phone; 120 s
  mason devices list | revoke ID      paired devices; revoking cuts one off at once
  mason usage [--account A] [--json]  each provider account: limit windows, overage, tokens and cost
  mason landing                       batches, phases, freezes, and the landing queue
  mason self                          MASON on its own repository: limits, and changes held for you
  mason proposals [--open] [--json]   what agents noticed: source, severity, route, state
  mason proposal show <id> | approve <id> | reject <id> [--reason R]
  mason proposal edit <id> [--title T] [--body B] [--evidence E]   only while it waits for you
  mason proposal file --task N --title T --body B --evidence E [--blocks]   as task N's agent would
  mason proposal publish <id> --preview | --confirm   send an improvement upstream, redacted
  mason proposal poll                 look at the upstream pull requests now
  mason improve \"<what to improve>\" [--project P]   ask MASON to change its own code
  mason update [--channel stable|main|branch:X | --sha S] [--rollback] [--check]
  mason update status | ack           the runtimes, a rollback alert, an update in progress
  mason config show [--project P]     every step's settings and where each came from
  mason config set <key> <value> --for 2h [--project P] [--owner O]
  mason config unset <key> [--project P] | list [--project P]
  mason help filing                   how to say a task is already explored

Global options: --data-dir D, --config-dir C (or MASON_DATA_DIR, MASON_CONFIG_DIR),
  --as NAME and --reason TEXT (who is acting and why; both optional, recorded with overrides)";

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

    fn peek_pos(&self) -> Option<&str> {
        self.rest.first().map(String::as_str)
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

/// `--pipeline NAME [--priority P] [--progress N]`: another pipeline's request.
fn outside_args(a: &mut Args) -> Option<resources::OutsideArgs> {
    let pipeline = a.opt("--pipeline");
    let priority = a.opt("--priority");
    let progress = a.opt("--progress").map(|p| {
        p.parse::<u32>()
            .unwrap_or_else(|_| die("--progress wants a whole number from 0 to 6"))
    });
    match pipeline {
        Some(pipeline) => Some(resources::OutsideArgs {
            pipeline,
            priority,
            progress,
        }),
        None if priority.is_some() || progress.is_some() => {
            die("--priority and --progress go with --pipeline NAME")
        }
        None => None,
    }
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

fn ago_until(ms: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    let s = ms.saturating_sub(now) / 1000;
    match s {
        0..=3599 => format!("in {}m", s / 60),
        3600..=172_799 => format!("in {}h", s / 3600),
        _ => format!("in {}d", s / 86_400),
    }
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
    // `mason ... | head` closes our stdout: leave quietly, as a command-line tool should,
    // instead of panicking on the failed print.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let text = info.to_string();
        if text.contains("Broken pipe") || text.contains("os error 32") {
            exit(0);
        }
        default_hook(info);
    }));
    let mut rest: Vec<String> = std::env::args().skip(1).collect();
    // Everything after `--` in `mason run` belongs to the command, flags included.
    let mut run_tail: Option<Vec<String>> = None;
    // (Global options may come first: `mason --data-dir D run ...`.)
    let mut i = 0;
    while i < rest.len() {
        match rest[i].as_str() {
            "--data-dir" | "--config-dir" => i += 2,
            "run" => {
                if let Some(j) = rest.iter().position(|x| x == "--") {
                    run_tail = Some(rest.split_off(j + 1));
                    rest.pop();
                }
                break;
            }
            f if f.starts_with("--") => i += 1,
            _ => break,
        }
    }
    let mut a = Args { rest };
    if a.rest.is_empty() || a.flag("--help") || a.flag("-h") {
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
    // Who is acting and why, for the record of interventions. Optional: a
    // required reason adds friction exactly when someone is in a hurry.
    // (`reject`, `skip`, `land-now`, `force` and `answer` read their own
    // `--reason` and `--as`; for the rest they are global.)
    let reads_own = a.rest.iter().any(|x| {
        matches!(
            x.as_str(),
            "reject" | "skip" | "land-now" | "force" | "answer"
        )
    });
    if !reads_own {
        if let Some(who) = a.opt("--as") {
            std::env::set_var("MASON_AS", who);
        }
        if let Some(why) = a.opt("--reason") {
            std::env::set_var("MASON_REASON", why);
        }
    } else {
        // Written before the command (`mason --reason R task skip ...`), they
        // are still global; after it, the command takes them itself.
        while matches!(
            a.rest.first().map(String::as_str),
            Some("--as" | "--reason")
        ) && a.rest.len() >= 2
        {
            let name = a.rest.remove(0);
            let value = a.rest.remove(0);
            std::env::set_var(
                if name == "--as" {
                    "MASON_AS"
                } else {
                    "MASON_REASON"
                },
                value,
            );
        }
    }
    let json_out = a.flag("--json");
    let cmd = a.pos().unwrap_or_default();

    if cmd == "help" {
        match a.pos().as_deref() {
            Some("filing") => print!("{}", mason_definition::filing::HELP),
            _ => println!("{HELP}"),
        }
        return;
    }

    if cmd == "hook" {
        let sub = a.pos().unwrap_or_default();
        let needs = a.opt("--needs").unwrap_or_else(|| "compile".into());
        finish(&a);
        if sub != "bash" {
            die("the only hook is `mason hook bash`");
        }
        let mut input = String::new();
        let _ = std::io::stdin().read_to_string(&mut input);
        // The rewritten command runs in a shell that may not have `mason` on its PATH.
        let mason = std::env::current_exe()
            .ok()
            .and_then(|p| p.to_str().map(String::from))
            .unwrap_or_else(|| "mason".into());
        if let Some(out) = resources::hook_bash(&input, &needs, &mason) {
            println!("{out}");
        }
        return;
    }

    // Reading a results file needs no daemon: it is the same judgment a suite gets.
    if cmd == "test" {
        let sub = a.pos().unwrap_or_default();
        if sub != "parse" {
            die("the only test command is `mason test parse <parser> <file> [--min N]`");
        }
        let min: u32 = a.opt("--min").and_then(|v| v.parse().ok()).unwrap_or(1);
        let kind = a.pos().unwrap_or_else(|| die("which parser?"));
        let file = a.pos().unwrap_or_else(|| die("which file?"));
        finish(&a);
        exit(testing::parse(&kind, std::path::Path::new(&file), min));
    }

    if cmd == "init" {
        exit(init::run(
            std::mem::take(&mut a.rest),
            &data_dir,
            &config_dir,
        ));
    }
    if cmd == "adopt" {
        exit(adopt::run(std::mem::take(&mut a.rest), &config_dir));
    }
    if cmd == "remove" {
        exit(remove::run(std::mem::take(&mut a.rest), &config_dir));
    }

    if cmd == "doctor" {
        finish(&a);
        let findings = doctor::run(&data_dir, &config_dir);
        let mut bad = false;
        for f in &findings {
            let mark = match f.level {
                doctor::Level::Ok => "ok  ",
                doctor::Level::Note => "note",
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

    // Looking at a definition needs no daemon.
    if cmd == "definition"
        && matches!(
            a.rest.first().map(String::as_str),
            Some("check" | "dry-run")
        )
    {
        let sub = a.pos().unwrap_or_default();
        let scenarios = a.opt("--scenarios").map(PathBuf::from);
        let file = PathBuf::from(a.pos().unwrap_or_else(|| ".mason/mason.toml".into()));
        finish(&a);
        exit(if sub == "check" {
            definition::check(&file)
        } else {
            definition::dry(&file, scenarios.as_deref())
        });
    }

    let d: Daemon = or_die(client::find(&data_dir));
    if cmd == "run" {
        let mut uses = resources::parse_needs(
            &a.opt("--needs")
                .unwrap_or_else(|| die("say what it needs: --needs compile")),
        )
        .unwrap_or_else(|e| die(&e));
        if a.flag("--exclusive") {
            uses.insert("exclusive".into(), 1);
        }
        let label = a.opt("--label");
        let outside = outside_args(&mut a);
        finish(&a);
        let command = run_tail.filter(|c| !c.is_empty()).unwrap_or_else(|| {
            die("give the command after `--`: mason run --needs compile -- cargo build")
        });
        exit(
            resources::run(&d, uses, label, &command, outside.as_ref()).unwrap_or_else(|e| die(&e)),
        );
    }
    if cmd == "hold" {
        let mut uses = resources::parse_needs(
            &a.opt("--needs")
                .unwrap_or_else(|| die("say what to hold: --needs llm:claude")),
        )
        .unwrap_or_else(|e| die(&e));
        if a.flag("--exclusive") {
            uses.insert("exclusive".into(), 1);
        }
        let label = a
            .opt("--label")
            .unwrap_or_else(|| die("name the hold: --label '#483 develop'"));
        let pid = a.opt("--pid").map(|p| {
            p.parse::<u32>()
                .unwrap_or_else(|_| die("--pid wants a process id"))
        });
        let outside =
            outside_args(&mut a).unwrap_or_else(|| die("name the pipeline: --pipeline starpipe"));
        finish(&a);
        resources::hold(&d, uses, &label, pid, &outside).unwrap_or_else(|e| die(&e));
        return;
    }
    if cmd == "targets" {
        match a.pos().as_deref() {
            Some("list") | None => {
                finish(&a);
                let r = or_die(d.call("GET", "/targets", None));
                if json_out {
                    return println!("{r:#}");
                }
                println!(
                    "Provider: {}{}",
                    r["provider"].as_str().unwrap_or("?"),
                    r["error"]
                        .as_str()
                        .map(|e| format!(" (last read failed: {e})"))
                        .unwrap_or_default()
                );
                let rows = r["devices"].as_array().cloned().unwrap_or_default();
                if rows.is_empty() {
                    println!("No devices are listed.");
                }
                for x in rows {
                    println!(
                        "{:<18} {:<40} {:<8} {:<12} {}",
                        x["kind"].as_str().unwrap_or("?"),
                        x["id"].as_str().unwrap_or("?"),
                        x["os"].as_str().unwrap_or(""),
                        x["state"].as_str().unwrap_or("?"),
                        x["leased_by"]
                            .as_str()
                            .map(|l| format!("leased by {l}"))
                            .unwrap_or_else(|| "free".into())
                    );
                }
                for p in r["pools"].as_array().cloned().unwrap_or_default() {
                    println!(
                        "{:<26} {} of {} in use",
                        p["name"].as_str().unwrap_or("?"),
                        p["used"],
                        p["units"]
                    );
                }
            }
            Some(other) => die(&format!("unknown targets command {other:?}\n\n{HELP}")),
        }
        return;
    }
    if cmd == "resources" {
        match a.pos().as_deref() {
            None => {
                finish(&a);
                resources::show(&d, json_out).unwrap_or_else(|e| die(&e));
            }
            Some("limit") => {
                let name = a
                    .pos()
                    .unwrap_or_else(|| die("which resource? e.g. llm:claude or compile"));
                let units: u32 = a
                    .pos()
                    .and_then(|u| u.parse().ok())
                    .unwrap_or_else(|| die("give the new limit as a whole number"));
                finish(&a);
                resources::limit(&d, &name, units).unwrap_or_else(|e| die(&e));
            }
            Some(other) => die(&format!("unknown resources command {other:?}\n\n{HELP}")),
        }
        return;
    }
    // The project a command is about: --project, or the only one there is.
    let project_of = |a: &mut Args| -> String {
        a.opt("--project").unwrap_or_else(|| {
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
        })
    };
    match cmd.as_str() {
        "editor" => {
            let sub = a.pos().unwrap_or_else(|| {
                die("editor what? show, op, undo, redo, discard, simulate or apply")
            });
            let project = project_of(&mut a);
            let step = a.opt("--step").unwrap_or_default();
            let message = a
                .opt("-m")
                .or_else(|| a.opt("--message"))
                .unwrap_or_default();
            let json_arg = if sub == "op" || sub == "simulate" {
                a.pos()
            } else {
                None
            };
            // Applying is the owner's act: say who.
            if sub == "apply" && std::env::var("MASON_AS").is_err() {
                if let Ok(u) = std::env::var("USER") {
                    std::env::set_var("MASON_AS", u);
                }
            }
            finish(&a);
            let base = format!("/projects/{project}/editor");
            let body = |s: &Option<String>| -> Option<Value> {
                s.as_ref().map(|t| {
                    serde_json::from_str(t)
                        .unwrap_or_else(|e| die(&format!("that is not JSON: {e}")))
                })
            };
            match sub.as_str() {
                "show" => {
                    let q = if step.is_empty() {
                        String::new()
                    } else {
                        format!("?step={step}")
                    };
                    let r = or_die(d.call("GET", &format!("{base}{q}"), None));
                    if json_out {
                        return println!("{r:#}");
                    }
                    for l in r["lanes"].as_array().into_iter().flatten() {
                        let cat = l["category"].as_str().unwrap_or("");
                        let nodes: Vec<String> = r["nodes"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter(|n| n["category"] == cat)
                            .map(|n| {
                                format!(
                                    "{} ({}){}",
                                    n["name"].as_str().unwrap_or(""),
                                    n["kind"].as_str().unwrap_or(""),
                                    if n["problems"].as_array().is_some_and(|p| !p.is_empty()) {
                                        " !"
                                    } else {
                                        ""
                                    }
                                )
                            })
                            .collect();
                        println!(
                            "{:<11} {}",
                            l["title"].as_str().unwrap_or(""),
                            nodes.join("   ")
                        );
                    }
                    for e in r["edges"].as_array().into_iter().flatten() {
                        println!(
                            "  {} -{}-> {}{}",
                            e["from"].as_str().unwrap_or(""),
                            if e["kind"] == "on_red" { "red" } else { "ok" },
                            e["to"].as_str().unwrap_or(""),
                            e["bound"]
                                .as_u64()
                                .map_or(String::new(), |b| format!(" (at most {b} times)"))
                        );
                    }
                    for n in r["nodes"].as_array().into_iter().flatten() {
                        for p in n["problems"].as_array().into_iter().flatten() {
                            println!(
                                "  PROBLEM on {}: {}",
                                n["name"].as_str().unwrap_or(""),
                                p.as_str().unwrap_or("")
                            );
                        }
                    }
                    for p in r["problems"].as_array().into_iter().flatten() {
                        println!("  PROBLEM: {}", p.as_str().unwrap_or(""));
                    }
                    println!(
                        "{}{}",
                        if r["dirty"] == true {
                            "draft changed"
                        } else {
                            "draft is the base"
                        },
                        if r["can_undo"] == true {
                            " (can undo)"
                        } else {
                            ""
                        }
                    );
                    for c in r["changes"].as_array().into_iter().flatten() {
                        println!("  this {}", c.as_str().unwrap_or(""));
                    }
                    for c in r["affected"].as_array().into_iter().flatten() {
                        println!("  {}", c.as_str().unwrap_or(""));
                    }
                    if let Some(diff) = r["diff"].as_str().filter(|d| !d.is_empty()) {
                        println!("{diff}");
                    }
                }
                "op" => {
                    let b = body(&json_arg).unwrap_or_else(|| die("give the operation as JSON"));
                    match d.call("POST", &format!("{base}/op"), Some(&b)) {
                        Ok(_) => println!("ok"),
                        Err(e) => die(&format!("refused: {e}")),
                    }
                }
                "undo" | "redo" | "discard" => {
                    let r = or_die(d.call("POST", &format!("{base}/{sub}"), Some(&json!({}))));
                    println!(
                        "{}",
                        if r["changed"] == true {
                            "done"
                        } else {
                            "nothing to do"
                        }
                    );
                }
                "simulate" => {
                    let r = or_die(d.call(
                        "POST",
                        &format!("{base}/simulate"),
                        Some(&body(&json_arg).unwrap_or(json!({}))),
                    ));
                    if json_out {
                        return println!("{r:#}");
                    }
                    for s in r["scenarios"].as_array().into_iter().flatten() {
                        let path: Vec<String> = s["steps"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .map(|x| {
                                format!(
                                    "{}{}",
                                    x["step"].as_str().unwrap_or(""),
                                    if x["outcome"] == "green" {
                                        String::new()
                                    } else {
                                        format!(" ({})", x["outcome"].as_str().unwrap_or(""))
                                    }
                                )
                            })
                            .collect();
                        println!(
                            "{:<44} {}  =>  {}",
                            s["name"].as_str().unwrap_or(""),
                            path.join(" > "),
                            s["end"]
                                .as_str()
                                .unwrap_or(s["error"].as_str().unwrap_or(""))
                        );
                    }
                }
                "apply" => {
                    let r = or_die(d.call(
                        "POST",
                        &format!("{base}/apply"),
                        Some(&json!({"message": message})),
                    ));
                    if r["applied"] == true {
                        println!("applied: {}", r["commit"].as_str().unwrap_or(""));
                    } else {
                        println!(
                            "{}: {}",
                            r["note"].as_str().unwrap_or("not applied"),
                            r["pull_request"].as_str().unwrap_or("")
                        );
                    }
                }
                other => die(&format!(
                    "editor {other}? use show, op, undo, redo, discard, simulate or apply"
                )),
            }
        }
        "extension" => {
            let sub = a
                .pos()
                .unwrap_or_else(|| die("extension what? list, show or approve"));
            let name = a.pos();
            let project = project_of(&mut a);
            // Approving is the owner's act: say who.
            if sub == "approve" && std::env::var("MASON_AS").is_err() {
                if let Ok(u) = std::env::var("USER") {
                    std::env::set_var("MASON_AS", u);
                }
            }
            finish(&a);
            let base = format!("/projects/{project}/extensions");
            let r = match sub.as_str() {
                "approve" => {
                    let n = name
                        .clone()
                        .unwrap_or_else(|| die("extension approve NAME"));
                    let r = or_die(d.call("POST", &format!("{base}/{n}/approve"), None));
                    if !json_out {
                        println!(
                            "approved {n} {} (by {})",
                            r["hash"].as_str().unwrap_or(""),
                            r["approved_by"].as_str().unwrap_or("")
                        );
                    }
                    r
                }
                "list" | "show" => or_die(d.call("GET", &base, None)),
                other => die(&format!("extension {other}? use list, show or approve")),
            };
            if json_out {
                return println!("{r:#}");
            }
            if sub != "approve" {
                for e in r["extensions"].as_array().into_iter().flatten() {
                    let n = e["name"].as_str().unwrap_or("");
                    if sub == "show" && name.as_deref().is_some_and(|w| w != n) {
                        continue;
                    }
                    println!(
                        "{n}  {}  {}  {}",
                        e["status"].as_str().unwrap_or(""),
                        e["hash"].as_str().unwrap_or(""),
                        e["provides"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                    if let Some(why) = e["refused"].as_str() {
                        println!("    refused: {why}");
                    }
                    if sub == "show" {
                        for f in e["files"].as_array().into_iter().flatten() {
                            println!("    {}", f.as_str().unwrap_or(""));
                        }
                    }
                }
            }
        }
        "definition" => {
            let sub = a
                .pos()
                .unwrap_or_else(|| die("definition what? check, dry-run, show or reload"));
            let project = project_of(&mut a);
            finish(&a);
            match sub.as_str() {
                "show" => {
                    let r = or_die(d.call("GET", &format!("/projects/{project}/definition"), None));
                    if json_out {
                        return println!("{r:#}");
                    }
                    println!(
                        "definition {} (from {}), start `{}`{}",
                        &r["hash"].as_str().unwrap_or("")
                            [..12.min(r["hash"].as_str().unwrap_or("").len())],
                        r["origin"].as_str().unwrap_or("?"),
                        r["start"].as_str().unwrap_or("?"),
                        r["explored_entry"]
                            .as_str()
                            .map(|e| format!(", skipping exploring starts at `{e}`"))
                            .unwrap_or_default()
                    );
                    for s in r["steps"].as_array().into_iter().flatten() {
                        let next = s["next"]
                            .as_array()
                            .map(|n| {
                                n.iter()
                                    .filter_map(Value::as_str)
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            })
                            .unwrap_or_default();
                        let red = s["on_red"]
                            .as_str()
                            .map(|r| {
                                format!("   on red back to `{r}` ({} rounds)", s["max_rounds"])
                            })
                            .unwrap_or_default();
                        println!(
                            "  {:<12} {:<9} {:<10} next: {}{red}",
                            s["name"].as_str().unwrap_or("?"),
                            s["type"].as_str().unwrap_or("?"),
                            s["category"].as_str().unwrap_or("?"),
                            if next.is_empty() {
                                "(ends)".into()
                            } else {
                                next
                            }
                        );
                    }
                    if let Some(errs) = r["error"].as_array() {
                        println!("\nthe latest edit was REFUSED, and the definition above keeps running:");
                        errs.iter()
                            .filter_map(Value::as_str)
                            .for_each(|e| println!("  {e}"));
                    }
                }
                "reload" => {
                    let r = or_die(d.call("POST", &format!("/projects/{project}/reload"), None));
                    match r["result"].as_str() {
                        Some("applied") => println!(
                            "Applied {} ({}); tasks admitted from now on use it",
                            &r["hash"].as_str().unwrap_or("")
                                [..12.min(r["hash"].as_str().unwrap_or("").len())],
                            r["origin"].as_str().unwrap_or("")
                        ),
                        Some("refused") => {
                            println!("Refused; the current definition keeps running:");
                            r["errors"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .filter_map(Value::as_str)
                                .for_each(|e| println!("  {e}"));
                            exit(1);
                        }
                        _ => println!("Nothing changed"),
                    }
                }
                other => die(&format!("unknown definition command {other:?}\n\n{HELP}")),
            }
        }
        "config" => {
            let sub = a
                .pos()
                .unwrap_or_else(|| die("config what? show, set, unset or list"));
            match sub.as_str() {
                "show" => {
                    let project = project_of(&mut a);
                    finish(&a);
                    let r = or_die(d.call("GET", &format!("/projects/{project}/config"), None));
                    if json_out {
                        return println!("{r:#}");
                    }
                    println!(
                        "settings for {project} (definition {})",
                        &r["definition"].as_str().unwrap_or("")
                            [..12.min(r["definition"].as_str().unwrap_or("").len())]
                    );
                    for s in r["steps"].as_array().into_iter().flatten() {
                        println!(
                            "  {} ({})",
                            s["step"].as_str().unwrap_or("?"),
                            s["type"].as_str().unwrap_or("?")
                        );
                        for k in [
                            "tier",
                            "level",
                            "model",
                            "effort",
                            "budget",
                            "max_rounds",
                            "command",
                            "build",
                        ] {
                            if let Some(v) = s[k]["value"].as_str() {
                                println!(
                                    "      {k:<11} {v}   <- {}",
                                    s[k]["from"].as_str().unwrap_or("?")
                                );
                            }
                        }
                    }
                }
                "set" => {
                    let secs = a
                        .opt("--for")
                        .map(|v| {
                            mason_definition::model::parse_duration(&v).unwrap_or_else(|| {
                                die(&format!("`{v}` is not a duration; use forms like 2h, 30m"))
                            })
                        })
                        .unwrap_or_else(|| {
                            die("a runtime override must expire: say how long with --for 2h")
                        });
                    let owner = a
                        .opt("--owner")
                        .or_else(|| std::env::var("USER").ok())
                        .unwrap_or_else(|| "owner".into());
                    let project = project_of(&mut a);
                    let key = a
                        .pos()
                        .unwrap_or_else(|| die("which setting? e.g. step.develop.model"));
                    let value = a.pos().unwrap_or_else(|| die("which value?"));
                    finish(&a);
                    or_die(d.call("POST", &format!("/projects/{project}/overrides"), Some(&json!({"key": key, "value": value, "owner": owner, "expires_in_s": secs.as_secs()}))));
                    println!(
                        "{project}: {key} = {value}, by {owner}, for {}s",
                        secs.as_secs()
                    );
                }
                "unset" => {
                    let project = project_of(&mut a);
                    let key = a.pos().unwrap_or_else(|| die("which setting?"));
                    finish(&a);
                    let r = or_die(d.call(
                        "POST",
                        &format!("/projects/{project}/overrides/remove"),
                        Some(&json!({"key": key})),
                    ));
                    println!("Removed {} override(s) of {key}", r["removed"]);
                }
                "list" => {
                    let project = project_of(&mut a);
                    finish(&a);
                    let r = or_die(d.call("GET", &format!("/projects/{project}/overrides"), None));
                    if json_out {
                        return println!("{r:#}");
                    }
                    let list = r["overrides"].as_array().cloned().unwrap_or_default();
                    if list.is_empty() {
                        println!("no runtime overrides in force");
                    }
                    for o in list {
                        println!(
                            "{} = {}   by {}, expires at {}",
                            o["key"].as_str().unwrap_or("?"),
                            o["value"].as_str().unwrap_or("?"),
                            o["owner"].as_str().unwrap_or("?"),
                            o["expires_at"]
                        );
                    }
                }
                other => die(&format!("unknown config command {other:?}\n\n{HELP}")),
            }
        }
        "pause" => match a.pos().as_deref() {
            Some("list") => {
                finish(&a);
                let r = or_die(d.call("GET", "/pauses", None));
                if json_out {
                    return println!("{r:#}");
                }
                let list = r["pauses"].as_array().cloned().unwrap_or_default();
                if list.is_empty() {
                    println!("nothing is paused");
                }
                for p in list {
                    println!(
                        "{} for {}, by {}, until {}",
                        p["mode"].as_str().unwrap_or("?"),
                        if p["project"].as_str().unwrap_or("").is_empty() {
                            "everything"
                        } else {
                            p["project"].as_str().unwrap_or("")
                        },
                        p["owner"].as_str().unwrap_or("?"),
                        ago_until(p["expires_at"].as_u64().unwrap_or(0))
                    );
                }
            }
            first => {
                // `mason pause --for 2h`: the first word was a flag, put it back.
                if let Some(f) = first {
                    a.rest.insert(0, f.to_string());
                }
                let project = a.opt("--project").unwrap_or_default();
                let mode = a.opt("--mode").unwrap_or_else(|| "all".into());
                let secs = a
                    .opt("--for")
                    .and_then(|v| mason_definition::model::parse_duration(&v))
                    .map(|d| d.as_secs())
                    .unwrap_or_else(|| die("a pause needs an expiry: --for 2h"));
                let owner = std::env::var("MASON_AS")
                    .or_else(|_| std::env::var("USER"))
                    .unwrap_or_default();
                finish(&a);
                or_die(d.call(
                    "POST",
                    "/pause",
                    Some(&json!({"project": project, "mode": mode, "for_s": secs, "owner": owner})),
                ));
                println!(
                    "Paused ({mode}) for {}, for {secs}s. `mason resume` lifts it. {}",
                    if project.is_empty() {
                        "everything"
                    } else {
                        &project
                    },
                    if mode == "all" {
                        "Running steps finish; nothing new starts."
                    } else {
                        ""
                    }
                );
            }
        },
        "account" => {
            let sub = a
                .pos()
                .unwrap_or_else(|| die("account hold <name> | account release <name>"));
            let name = a.pos().unwrap_or_else(|| die("which account?"));
            let by = std::env::var("MASON_AS")
                .or_else(|_| std::env::var("USER"))
                .unwrap_or_default();
            match sub.as_str() {
                "hold" => {
                    let for_s = a
                        .opt("--for")
                        .and_then(|v| mason_definition::model::parse_duration(&v))
                        .map(|d| d.as_secs());
                    let reason = a.opt("--reason").unwrap_or_default();
                    finish(&a);
                    or_die(d.call(
                        "POST",
                        "/account/hold",
                        Some(&json!({"account": name, "for_s": for_s, "reason": reason, "by": by})),
                    ));
                    println!("Held {name}: no new session starts on it. `mason account release {name}` lifts it.");
                }
                "release" => {
                    finish(&a);
                    let r = or_die(d.call(
                        "POST",
                        "/account/release",
                        Some(&json!({"account": name, "by": by})),
                    ));
                    if r["released"].as_bool().unwrap_or(false) {
                        println!("Released {name}: waiting tasks try again now.");
                    } else {
                        println!("{name} was not held.");
                    }
                }
                other => die(&format!("unknown account command {other:?}\n\n{HELP}")),
            }
        }
        "resume" => {
            let project = a.opt("--project").unwrap_or_default();
            let mode = a.opt("--mode").unwrap_or_default();
            finish(&a);
            let r = or_die(d.call(
                "POST",
                "/resume",
                Some(&json!({"project": project, "mode": mode})),
            ));
            println!("Lifted {} pause(s)", r["lifted"]);
        }
        "lan" => {
            if !matches!(a.pos().as_deref(), Some("status") | None) {
                die("usage: mason lan status");
            }
            finish(&a);
            let r = or_die(d.call("GET", "/lan", None));
            if json_out {
                return println!("{r:#}");
            }
            if r["enabled"].as_bool() == Some(true) {
                println!(
                    "LAN access is on: port {}, fingerprint {}",
                    r["port"],
                    r["fingerprint"].as_str().unwrap_or("?")
                );
            } else {
                println!("LAN access is off (loopback only). Turn it on with `[lan] enabled = true` in daemon.toml, or `masond --lan`.");
            }
        }
        "pair" => {
            let role = if a.flag("--viewer") {
                "viewer"
            } else {
                "owner"
            };
            finish(&a);
            let r = or_die(d.call("POST", "/pair/codes", Some(&json!({"role": role}))));
            if json_out {
                return println!("{r:#}");
            }
            println!(
                "Pairing code ({role}, valid {} s, one use): {}\n{}",
                r["expires_in_s"],
                r["code"].as_str().unwrap_or("?"),
                r["url"].as_str().unwrap_or("?")
            );
        }
        "devices" => match a.pos().as_deref() {
            Some("list") | None => {
                finish(&a);
                let r = or_die(d.call("GET", "/devices", None));
                if json_out {
                    return println!("{r:#}");
                }
                let rows = r["devices"].as_array().cloned().unwrap_or_default();
                if rows.is_empty() {
                    println!("No devices are paired.");
                }
                for x in rows {
                    println!(
                        "#{} {:<20} {:<7} {}",
                        x["id"],
                        x["name"].as_str().unwrap_or("?"),
                        x["role"].as_str().unwrap_or("?"),
                        if x["revoked_at"].is_null() {
                            "paired"
                        } else {
                            "revoked"
                        }
                    );
                }
            }
            Some("revoke") => {
                let id: u64 = a
                    .pos()
                    .and_then(|v| v.trim_start_matches('#').parse().ok())
                    .unwrap_or_else(|| die("usage: mason devices revoke ID"));
                finish(&a);
                or_die(d.call("POST", &format!("/devices/{id}/revoke"), None));
                println!("Revoked device #{id}; its token no longer works.");
            }
            Some(other) => die(&format!("unknown devices command {other:?}\n\n{HELP}")),
        },
        "notify" => {
            match a.pos().as_deref() {
                Some("test") => {
                    let Some(channel) = a.pos() else {
                        die("mason notify test CHANNEL  (sound, speech, push, email, webhook)")
                    };
                    finish(&a);
                    let r =
                        or_die(d.call("POST", "/notify/test", Some(&json!({"channel": channel}))));
                    if json_out {
                        return println!("{r:#}");
                    }
                    println!("{r}");
                }
                Some("status") | None => {
                    finish(&a);
                    let r = or_die(d.call("GET", "/notify", None));
                    if json_out {
                        return println!("{r:#}");
                    }
                    if let Some(e) = r["error"].as_str() {
                        println!("notify.toml has a problem (defaults are in force): {e}");
                    }
                    for c in r["routing"].as_array().cloned().unwrap_or_default() {
                        let rules: Vec<String> = c["rules"]
                            .as_array()
                            .cloned()
                            .unwrap_or_default()
                            .iter()
                            .map(|x| match x["after_s"].as_u64().unwrap_or(0) {
                                0 => x["channel"].as_str().unwrap_or("?").to_string(),
                                s => format!(
                                    "{} after {} min",
                                    x["channel"].as_str().unwrap_or("?"),
                                    s / 60
                                ),
                            })
                            .collect();
                        println!(
                            "{:<16} {}",
                            c["condition"].as_str().unwrap_or("?"),
                            rules.join(", ")
                        );
                    }
                    match r["last_played"].as_u64() {
                        Some(t) => println!("last announcement played at {t} (ms since the epoch)"),
                        None => println!("no announcement has been played yet"),
                    }
                    println!(
                    "one reminder after {} min; daemon speech {}; email {}, push {}, webhook {}",
                    r["reminder_after_s"].as_u64().unwrap_or(0) / 60,
                    if r["daemon_speech"].as_bool().unwrap_or(false) { "on" } else { "off" },
                    if r["configured"]["email"].as_bool().unwrap_or(false) { "set up" } else { "not set up" },
                    if r["configured"]["push"].as_bool().unwrap_or(false) { "set up" } else { "not set up" },
                    if r["configured"]["webhook"].as_bool().unwrap_or(false) { "set up" } else { "not set up" },
                );
                    if r["reaches_without_monitor"].as_bool() == Some(false) {
                        println!("{}", mason_daemon::notify::NO_REACH_LINE);
                    }
                }
                Some(other) => die(&format!("unknown notify command {other:?}\n\n{HELP}")),
            }
        }
        "alerts" => {
            if a.peek_pos() == Some("ack") {
                a.pos();
                let Some(key) = a.pos() else {
                    die("mason alerts ack KEY")
                };
                finish(&a);
                or_die(d.call("POST", "/alerts/ack", Some(&json!({"key": key}))));
                return println!(
                    "Acknowledged {key}: it stays listed while it is true, and stops reminding."
                );
            }
            let all = a.flag("--all");
            finish(&a);
            let r = or_die(d.call("GET", &format!("/alerts?all={}", u8::from(all)), None));
            if json_out {
                return println!("{r:#}");
            }
            let list = r["alerts"].as_array().cloned().unwrap_or_default();
            if let Some(note) = r["note"].as_str() {
                println!("note: {note}");
            }
            if list.is_empty() {
                println!("nothing needs a look");
            }
            for x in list {
                println!(
                    "{} {:<12} {}{}  [{}]",
                    if x["cleared_at"].is_null() { "!" } else { "." },
                    x["kind"].as_str().unwrap_or("?"),
                    x["text"].as_str().unwrap_or(""),
                    if x["acked_at"].is_null() {
                        ""
                    } else {
                        " (acknowledged)"
                    },
                    x["key"].as_str().unwrap_or("")
                );
            }
        }
        "health" => {
            finish(&a);
            let r = or_die(d.call("GET", "/health", None));
            if json_out {
                return println!("{r:#}");
            }
            print_health(&r);
        }
        "digest" => {
            let days = a.opt("--days").unwrap_or_else(|| "7".into());
            let project = a.opt("--project");
            finish(&a);
            let mut path = format!("/digest?days={days}");
            if let Some(p) = project {
                path.push_str(&format!("&project={p}"));
            }
            let r = or_die(d.call("GET", &path, None));
            if json_out {
                return println!("{r:#}");
            }
            print_digest(&r);
        }
        "schedule" => {
            let sub = a.pos().unwrap_or_default();
            if std::env::var("MASON_AS").is_err() {
                if let Ok(u) = std::env::var("USER") {
                    std::env::set_var("MASON_AS", u);
                }
            }
            match sub.as_str() {
                "list" => {
                    finish(&a);
                    let r = or_die(d.call("GET", "/schedules", None));
                    if json_out {
                        return println!("{r:#}");
                    }
                    for s in r["schedules"].as_array().cloned().unwrap_or_default() {
                        println!(
                            "{:<28} {:<12} every {:<10} runs {:<4} {}",
                            s["name"].as_str().unwrap_or("?"),
                            s["kind"].as_str().unwrap_or("?"),
                            every_text(s["every_ms"].as_u64().unwrap_or(0)),
                            s["runs"],
                            s["last_status"].as_str().unwrap_or("")
                        );
                    }
                }
                "run" => {
                    let name = a
                        .pos()
                        .unwrap_or_else(|| die("usage: mason schedule run <name>"));
                    finish(&a);
                    let r = or_die(d.call("POST", "/schedules/run", Some(&json!({"name": name}))));
                    println!("{}", r["said"].as_str().unwrap_or(""));
                }
                _ => die("usage: mason schedule list | run <name>"),
            }
        }
        "triage" => {
            let sub = a.pos().unwrap_or_default();
            if std::env::var("MASON_AS").is_err() {
                if let Ok(u) = std::env::var("USER") {
                    std::env::set_var("MASON_AS", u);
                }
            }
            match sub.as_str() {
                "list" => {
                    finish(&a);
                    let r = or_die(d.call("GET", "/triage", None));
                    if json_out {
                        return println!("{r:#}");
                    }
                    let list = r["triage"].as_array().cloned().unwrap_or_default();
                    if list.is_empty() {
                        println!("no triage suggestions (triage is off unless a project sets [triage] enabled)");
                    }
                    for t in list {
                        println!(
                            "#{:<4} task {:<4} {:<9} {:<9} {}\n      would {}: {}",
                            t["id"],
                            t["task"],
                            t["status"].as_str().unwrap_or("?"),
                            t["verdict"].as_str().unwrap_or(""),
                            t["question"].as_str().unwrap_or(""),
                            t["action"].as_str().unwrap_or("?"),
                            t["answer"].as_str().unwrap_or("")
                        );
                    }
                }
                "review" => {
                    let id = task_id(&mut a);
                    let agree = a.flag("--agree");
                    let disagree = a.flag("--disagree");
                    finish(&a);
                    if agree == disagree {
                        die("say --agree or --disagree");
                    }
                    or_die(d.call(
                        "POST",
                        &format!("/triage/{id}/review"),
                        Some(&json!({"agree": agree})),
                    ));
                    println!(
                        "reviewed #{id}: {}",
                        if agree { "agree" } else { "disagree" }
                    );
                }
                _ => die("usage: mason triage list | review <id> (--agree | --disagree)"),
            }
        }
        "metrics" => {
            let days = a.opt("--days").unwrap_or_else(|| "0".into());
            finish(&a);
            let r = or_die(d.call("GET", &format!("/metrics?days={days}"), None));
            if json_out {
                return println!("{r:#}");
            }
            println!(
                "{} intervention(s) for {} landed task(s): {} per landed task",
                r["interventions"],
                r["landed"],
                r["per_landed"]
                    .as_f64()
                    .map_or("n/a".to_string(), |v| format!("{v:.2}"))
            );
            if let Some(k) = r["by_kind"].as_object() {
                for (kind, n) in k {
                    println!("  {kind:<16} {n}");
                }
            }
            if r["acknowledged"].as_u64().unwrap_or(0) > 0 {
                println!(
                    "  ({} question(s) acknowledged rather than asked again)",
                    r["acknowledged"]
                );
            }
            if r["refused_answers"].as_u64().unwrap_or(0) > 0 {
                println!(
                    "  ({} answer(s) refused: not the owner's identity)",
                    r["refused_answers"]
                );
            }
        }
        "update" => {
            // `status` and `ack` are words; everything else is flags for an update.
            let sub = match a.rest.first().map(String::as_str) {
                Some(w @ ("status" | "ack")) => {
                    let w = w.to_string();
                    a.rest.remove(0);
                    Some(w)
                }
                _ => None,
            };
            match sub.as_deref() {
                Some("status") => {
                    finish(&a);
                    let r = or_die(d.call("GET", "/update", None));
                    if json_out {
                        return println!("{r:#}");
                    }
                    println!(
                        "running   {}",
                        r["running"]
                            .as_str()
                            .unwrap_or("(not from a runtime: a build directory)")
                    );
                    println!("current   {}", r["current"].as_str().unwrap_or("-"));
                    println!("previous  {}", r["previous"].as_str().unwrap_or("-"));
                    println!(
                        "channel   {}",
                        r["channel"].as_str().unwrap_or("(no update.toml)")
                    );
                    if r["checked_at_ms"].as_u64().unwrap_or(0) > 0 {
                        println!(
                            "latest    {}{}",
                            r["latest"].as_str().unwrap_or("-"),
                            if r["outdated"].as_bool().unwrap_or(false) {
                                format!("  (masond is {} commit(s) behind)", r["behind"])
                            } else {
                                String::new()
                            }
                        );
                    }
                    if let Some(e) = r["check_error"].as_str() {
                        println!("check     could not look for a newer version: {e}");
                    }
                    if let Some(b) = r["bad"].as_array().filter(|b| !b.is_empty()) {
                        println!(
                            "bad       {}",
                            b.iter()
                                .filter_map(|x| x.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        );
                    }
                    if let Some(al) = r["alert"].as_str() {
                        println!("ALERT     {al}  (`mason update ack` when you have seen it)");
                    }
                    println!(
                        "update    {}{}",
                        r["phase"].as_str().unwrap_or("?"),
                        r["error"]
                            .as_str()
                            .map_or(String::new(), |e| format!(": {e}"))
                    );
                }
                Some("ack") => {
                    finish(&a);
                    or_die(d.call("POST", "/update/ack", Some(&json!({}))));
                    println!("acknowledged");
                }
                _ => {
                    // `mason update [--channel C | --sha S] [--rollback] [--check]`
                    let channel = a.opt("--channel");
                    let sha = a.opt("--sha");
                    let rollback = a.flag("--rollback");
                    let check = a.flag("--check");
                    finish(&a);
                    let mut body = json!({"rollback": rollback, "check": check});
                    if let Some(c) = channel {
                        body["channel"] = json!(c);
                    }
                    if let Some(s) = sha {
                        body["sha"] = json!(s);
                    }
                    or_die(d.call("POST", "/update", Some(&body)));
                    // Follow it until it finishes, or the daemon is replaced.
                    let (mut shown, mut seen_down) = (0usize, false);
                    // The daemon logs "taking over" just before it execs, and its log does not
                    // survive the exec: a poll may never see that line.
                    let mut said_takeover = false;
                    // The runtime running when the update began; another one at the end took over.
                    // (`None` until the first answer; an inner `None` is a build directory.)
                    let mut first_running: Option<Option<String>> = None;
                    let mut target: Option<String> = None;
                    let patience: u64 = std::env::var("MASON_UPDATE_WAIT_S")
                        .ok()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(30);
                    let mut down_since: Option<std::time::Instant> = None;
                    let end = std::time::Instant::now() + std::time::Duration::from_secs(3600);
                    loop {
                        std::thread::sleep(std::time::Duration::from_millis(500));
                        if std::time::Instant::now() > end {
                            die("gave up waiting after an hour");
                        }
                        let Ok(r) = d.call("GET", "/update", None) else {
                            seen_down = true; // the daemon is being replaced
                            let since = *down_since.get_or_insert_with(std::time::Instant::now);
                            if since.elapsed().as_secs() > patience {
                                die("the daemon went away while taking over and has not come back. If the new build is crashing it is rolled back by itself after three crashes; `mason update status` shows where things stand once a daemon answers");
                            }
                            continue;
                        };
                        down_since = None;
                        if first_running.is_none() {
                            first_running = Some(r["running"].as_str().map(str::to_string));
                        }
                        let log: Vec<&str> = r["log"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|l| l.as_str())
                            .collect();
                        // A daemon that took over starts a new log.
                        if log.len() < shown {
                            shown = 0;
                        }
                        for l in log.iter().skip(shown) {
                            println!("  {l}");
                            said_takeover |= l.starts_with("taking over");
                            shown += 1;
                        }
                        if let Some(t) = r["target"].as_str() {
                            target = Some(t.to_string());
                        }
                        let phase = r["phase"].as_str().unwrap_or("").to_string();
                        if phase == "failed" {
                            die(&format!(
                                "the update failed, and the running daemon is unchanged: {}",
                                r["error"].as_str().unwrap_or("")
                            ));
                        }
                        // Done when it is idle and either only checking, or the new runtime is the
                        // one that was being built (a restarted daemon has forgotten the target).
                        if phase == "idle"
                            && (check
                                || (target.is_some() && r["running"].as_str() == target.as_deref()))
                        {
                            let running = r["running"].as_str().unwrap_or("(a build directory)");
                            let changed = first_running
                                .as_ref()
                                .is_some_and(|f| f.as_deref() != r["running"].as_str());
                            if !check && !said_takeover && (changed || seen_down) {
                                println!(
                                    "  taking over: {running} took over; step hosts keep running"
                                );
                            }
                            println!("running {running}");
                            break;
                        }
                    }
                }
            }
        }
        "proposals" => {
            let open = a.flag("--open");
            finish(&a);
            let r = or_die(d.call(
                "GET",
                if open {
                    "/proposals?open=1"
                } else {
                    "/proposals"
                },
                None,
            ));
            if json_out {
                return println!("{r:#}");
            }
            let list = r["proposals"].as_array().cloned().unwrap_or_default();
            if list.is_empty() {
                println!("no proposals");
            }
            for p in list {
                println!(
                    "#{:<4} {:<9} {:<8} {:<7} {:<9} {}\n      {}",
                    p["task"],
                    p["state"].as_str().unwrap_or("?"),
                    p["source"].as_str().unwrap_or("?"),
                    p["severity"].as_str().unwrap_or("?"),
                    p["route"].as_str().unwrap_or("?"),
                    p["title"].as_str().unwrap_or(""),
                    p["project"].as_str().unwrap_or("")
                );
            }
        }
        "proposal" => {
            let sub = a.pos().unwrap_or_default();
            if std::env::var("MASON_AS").is_err() {
                if let Ok(u) = std::env::var("USER") {
                    std::env::set_var("MASON_AS", u);
                }
            }
            match sub.as_str() {
                "show" => {
                    let id = task_id(&mut a);
                    finish(&a);
                    let p = or_die(d.call("GET", &format!("/proposals/{id}"), None));
                    if json_out {
                        return println!("{p:#}");
                    }
                    println!(
                        "#{id} {} ({}, {}, routed to {})",
                        p["title"].as_str().unwrap_or(""),
                        p["source"].as_str().unwrap_or(""),
                        p["severity"].as_str().unwrap_or(""),
                        p["route"].as_str().unwrap_or("")
                    );
                    println!(
                        "state: {}   publish: {}",
                        p["state"].as_str().unwrap_or(""),
                        p["publish_state"].as_str().unwrap_or("")
                    );
                    println!(
                        "\n{}\n\nEvidence:\n{}",
                        p["body"].as_str().unwrap_or(""),
                        p["evidence"].as_str().unwrap_or("(none)")
                    );
                    if !p["branch"].as_str().unwrap_or("").is_empty() {
                        println!(
                            "\nbranch {} ({} access){}",
                            p["branch"].as_str().unwrap_or(""),
                            p["access"].as_str().unwrap_or(""),
                            p["pr"].as_u64().map_or(String::new(), |n| format!(
                                ", upstream pull request #{n} ({})",
                                p["pr_state"].as_str().unwrap_or("")
                            ))
                        );
                    }
                }
                "approve" => {
                    let id = task_id(&mut a);
                    finish(&a);
                    or_die(d.call(
                        "POST",
                        &format!("/proposals/{id}/approve"),
                        Some(&json!({})),
                    ));
                    println!("Approved proposal #{id}");
                }
                "reject" => {
                    let id = task_id(&mut a);
                    let reason = a
                        .opt("--reason")
                        .unwrap_or_else(|| "no reason given".into());
                    finish(&a);
                    or_die(d.call(
                        "POST",
                        &format!("/proposals/{id}/reject"),
                        Some(&json!({"reason": reason})),
                    ));
                    println!("Rejected proposal #{id}");
                }
                "edit" => {
                    let id = task_id(&mut a);
                    let mut body = json!({});
                    for k in ["title", "body", "evidence"] {
                        if let Some(v) = a.opt(&format!("--{k}")) {
                            body[k] = json!(v);
                        }
                    }
                    finish(&a);
                    if body.as_object().is_some_and(|o| o.is_empty()) {
                        die("change something: --title, --body or --evidence");
                    }
                    or_die(d.call("POST", &format!("/proposals/{id}/edit"), Some(&body)));
                    println!("Edited proposal #{id}");
                }
                "file" => {
                    let task = a.opt("--task").map(|t| {
                        t.trim_start_matches('#')
                            .parse::<u64>()
                            .unwrap_or_else(|_| die("--task is a task number"))
                    });
                    let project = a.opt("--project");
                    let blocks = a.flag("--blocks");
                    let (title, body, evidence) =
                        (a.opt("--title"), a.opt("--body"), a.opt("--evidence"));
                    finish(&a);
                    let r = or_die(d.call(
                        "POST",
                        "/proposals",
                        Some(&json!({"task": task, "project": project, "title": title, "body": body, "evidence": evidence, "blocks_task": blocks})),
                    ));
                    println!(
                        "Filed #{} in {} ({}), {}",
                        r["task"],
                        r["project"].as_str().unwrap_or("?"),
                        r["route"].as_str().unwrap_or("?"),
                        if r["proposed"].as_bool().unwrap_or(true) {
                            "waiting for you"
                        } else {
                            "queued"
                        }
                    );
                }
                "publish" => {
                    let id = task_id(&mut a);
                    let (preview, confirm) = (a.flag("--preview"), a.flag("--confirm"));
                    finish(&a);
                    if preview == confirm {
                        die("say --preview (show what would be sent) or --confirm (send it)");
                    }
                    let r = or_die(d.call(
                        "POST",
                        &format!("/proposals/{id}/publish"),
                        Some(&json!({"confirm": confirm})),
                    ));
                    if json_out {
                        return println!("{r:#}");
                    }
                    println!(
                        "{}\n\n{}\n",
                        r["title"].as_str().unwrap_or(""),
                        r["body"].as_str().unwrap_or("")
                    );
                    println!(
                        "head {} -> base {}; {} redaction(s); attachments: none",
                        r["head"].as_str().unwrap_or(""),
                        r["base"].as_str().unwrap_or(""),
                        r["redactions"]
                    );
                    for f in r["files"].as_array().into_iter().flatten() {
                        println!("  changes {}", f.as_str().unwrap_or(""));
                    }
                    if let Some(b) = r["blocked"].as_str() {
                        println!("NOT PUBLISHABLE: {b}");
                    } else if confirm {
                        println!("Opened pull request #{} upstream", r["pr"]);
                    } else {
                        println!("Nothing has been sent. To send exactly this: mason proposal publish {id} --confirm");
                    }
                }
                "poll" => {
                    finish(&a);
                    or_die(d.call("POST", "/proposals/poll", Some(&json!({}))));
                    println!("Looked at the upstream pull requests");
                }
                _ => die(&format!("unknown proposal command {sub:?}\n\n{HELP}")),
            }
        }
        "improve" => {
            let project = a.opt("--project");
            let text = a
                .pos()
                .unwrap_or_else(|| die("say what to improve: mason improve \"<text>\""));
            finish(&a);
            if std::env::var("MASON_AS").is_err() {
                if let Ok(u) = std::env::var("USER") {
                    std::env::set_var("MASON_AS", u);
                }
            }
            let project = project.unwrap_or_else(|| {
                let r = or_die(d.call("GET", "/self", None));
                r["projects"]
                    .as_array()
                    .and_then(|l| l.first())
                    .and_then(|p| p["project"].as_str())
                    .map(String::from)
                    .unwrap_or_else(|| die("no project is MASON's own (a project file with a [self] section makes it so)"))
            });
            let r = or_die(d.call(
                "POST",
                &format!("/projects/{project}/improve"),
                Some(&json!({"text": text})),
            ));
            if json_out {
                return println!("{r:#}");
            }
            println!(
                "Filed #{} in {project}: branch {} ({} access)",
                r["task"],
                r["branch"].as_str().unwrap_or("?"),
                r["access"].as_str().unwrap_or("?")
            );
        }
        "self" => {
            finish(&a);
            let r = or_die(d.call("GET", "/self", None));
            if json_out {
                return println!("{r:#}");
            }
            let list = r["projects"].as_array().cloned().unwrap_or_default();
            if list.is_empty() {
                println!(
                    "no project is MASON's own (a project file with a [self] section makes it so)"
                );
            }
            for p in list {
                let held = p["held"].as_u64().unwrap_or(0);
                let held = if held > 0 {
                    format!(" ({held} held behind others, not counted)")
                } else {
                    String::new()
                };
                println!(
                    "{}: {}/{} open{} ({} of yours open, not limited), {} of {} this week ({} of yours not counted), {} of {} agent steps in {} h ({} of yours not counted){}",
                    p["project"].as_str().unwrap_or("?"),
                    p["open"],
                    p["max_open"],
                    held,
                    p["yours_open"].as_u64().unwrap_or(0),
                    p["week"],
                    p["per_week"],
                    p["yours_not_counted"].as_u64().unwrap_or(0),
                    p["steps"],
                    p["max_steps"],
                    p["period_h"],
                    p["yours_steps"].as_u64().unwrap_or(0),
                    p["blocked"]
                        .as_str()
                        .map_or(String::new(), |b| format!("\n  filing is blocked: {b}"))
                );
            }
            for h in r["holds"].as_array().into_iter().flatten() {
                println!(
                    "task #{:<4} {}: {}",
                    h["task"],
                    if h["approved_by"].is_null() {
                        "WAITING FOR YOU"
                    } else {
                        "approved"
                    },
                    h["reasons"].as_str().unwrap_or("")
                );
            }
        }
        "usage" => {
            let account = a.opt("--account");
            finish(&a);
            let mut r = or_die(d.call("GET", "/usage", None));
            if let Some(acc) = &account {
                let keep: Vec<Value> = r["providers"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|p| p["name"].as_str() == Some(acc.as_str()))
                    .cloned()
                    .collect();
                if keep.is_empty() {
                    die(&format!("no account named {acc}"));
                }
                r["providers"] = Value::Array(keep);
                r["topBar"] = Value::Array(
                    r["topBar"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|l| {
                            l.as_str()
                                .is_some_and(|l| l.starts_with(&format!("{acc} ")))
                        })
                        .cloned()
                        .collect(),
                );
            }
            if json_out {
                return println!("{r:#}");
            }
            print!("{}", usage_text(&r));
        }
        "landing" => {
            finish(&a);
            let r = or_die(d.call("GET", "/landing", None));
            if json_out {
                return println!("{r:#}");
            }
            for p in r["projects"].as_array().into_iter().flatten() {
                let c = &p["config"];
                if let Some(e) = c["error"].as_str() {
                    println!("{}: {e}", p["project"].as_str().unwrap_or("?"));
                    continue;
                }
                println!(
                    "{}: {} (window {}s, max {}, {} at once); {} queued, {} landing, queue idle {} s",
                    p["project"].as_str().unwrap_or("?"),
                    c["mode"].as_str().unwrap_or("?"),
                    c["window_s"],
                    c["max_size"],
                    c["parallel"],
                    p["queued"].as_array().map_or(0, Vec::len),
                    p["running_batches"],
                    p["queue_idle_ms"].as_u64().unwrap_or(0) / 1000
                );
            }
            for f in r["freezes"].as_array().into_iter().flatten() {
                println!(
                    "FROZEN {}: {} (fix: task #{})",
                    f["project"].as_str().unwrap_or("?"),
                    f["reason"].as_str().unwrap_or(""),
                    f["fix_task"]
                );
            }
            for b in r["batches"].as_array().into_iter().flatten().take(15) {
                println!(
                    "batch #{:<3} {:<10} tasks [{}] landed [{}] parked [{}] {}",
                    b["id"],
                    b["phase"].as_str().unwrap_or("?"),
                    b["tasks"].as_str().unwrap_or(""),
                    b["landed"].as_str().unwrap_or(""),
                    b["parked"].as_str().unwrap_or(""),
                    b["note"].as_str().unwrap_or("")
                );
            }
        }
        "debt" => match a.pos().as_deref() {
            Some("list") | None => {
                let all = a.flag("--all");
                finish(&a);
                let r = or_die(d.call("GET", &format!("/debts?open={}", u8::from(!all)), None));
                if json_out {
                    return println!("{r:#}");
                }
                let list = r["debts"].as_array().cloned().unwrap_or_default();
                if list.is_empty() {
                    println!("no debts");
                }
                for x in list {
                    println!(
                        "#{:<3} task #{:<4} {:<22} {}{}  {}",
                        x["id"],
                        x["task"],
                        x["scope"].as_str().unwrap_or("?"),
                        x["status"].as_str().unwrap_or("?"),
                        if x["deferred"].as_bool().unwrap_or(false) {
                            " (deferred)"
                        } else {
                            ""
                        },
                        x["reason"].as_str().unwrap_or("")
                    );
                }
            }
            Some("run") => {
                let id: u64 = a
                    .pos()
                    .and_then(|s| s.trim_start_matches('#').parse().ok())
                    .unwrap_or_else(|| die("which debt? give its number"));
                finish(&a);
                let r = or_die(d.call("POST", &format!("/debts/{id}/run"), None));
                println!("{}", r["detail"].as_str().unwrap_or(""));
                if r["paid"] != json!(true) {
                    exit(1);
                }
            }
            Some(other) => die(&format!("unknown debt command {other:?}\n\n{HELP}")),
        },
        "quarantine" => match a.pos().as_deref() {
            Some("list") | None => {
                finish(&a);
                let r = or_die(d.call("GET", "/quarantine", None));
                if json_out {
                    return println!("{r:#}");
                }
                let list = r["quarantined"].as_array().cloned().unwrap_or_default();
                if list.is_empty() {
                    println!("nothing is quarantined");
                }
                for x in list {
                    println!(
                        "{} :: {}   owner {}, expires {}   {}",
                        x["suite"].as_str().unwrap_or("?"),
                        x["test"].as_str().unwrap_or("?"),
                        x["owner"].as_str().unwrap_or("?"),
                        ago_until(x["expires_at"].as_u64().unwrap_or(0)),
                        x["reason"].as_str().unwrap_or("")
                    );
                }
                for x in r["candidates"].as_array().into_iter().flatten() {
                    println!(
                        "candidate: {} :: {} flaked 3 or more times this week",
                        x["suite"].as_str().unwrap_or("?"),
                        x["test"].as_str().unwrap_or("?")
                    );
                }
            }
            Some("add") => {
                let owner = a
                    .opt("--owner")
                    .unwrap_or_else(|| die("a quarantine needs an owner: --owner NAME"));
                let days: u64 = a
                    .opt("--days")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_else(|| die("a quarantine needs an expiry: --days N"));
                let reason = a.opt("--reason").unwrap_or_default();
                let suite = a.pos().unwrap_or_else(|| die("which suite?"));
                let test = a.pos().unwrap_or_else(|| die("which test?"));
                finish(&a);
                or_die(d.call(
                    "POST",
                    "/quarantine",
                    Some(&json!({"suite": suite, "test": test, "owner": owner, "days": days, "reason": reason})),
                ));
                println!("Quarantined {suite} :: {test} for {days} day(s), owned by {owner}");
            }
            Some("remove") => {
                let suite = a.pos().unwrap_or_else(|| die("which suite?"));
                let test = a.pos().unwrap_or_else(|| die("which test?"));
                finish(&a);
                let r = or_die(d.call(
                    "POST",
                    "/quarantine/remove",
                    Some(&json!({"suite": suite, "test": test})),
                ));
                println!("Removed {} quarantine(s)", r["removed"]);
            }
            Some(other) => die(&format!("unknown quarantine command {other:?}\n\n{HELP}")),
        },
        "status" => {
            let watch = a.flag("--watch") || a.flag("-w");
            finish(&a);
            if watch {
                // Every running step's progress, as it is pushed.
                or_die(progress::watch(&d, None, json_out).map_err(CallError::Io));
                return;
            }
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
            if let Some(st) = s["stuck"].as_object() {
                let n = st["tasks_waiting"].as_u64().unwrap_or(0);
                println!(
                    "STUCK: no new compile or agent session can start: disk has {} GB free, floor {} GB; {n} task{} waiting",
                    st["free_mb"].as_u64().unwrap_or(0) / 1024,
                    st["floor_mb"].as_u64().unwrap_or(0) / 1024,
                    if n == 1 { "" } else { "s" }
                );
            }
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
            if let Ok(r) = d.call("GET", "/resources", None) {
                for l in resources::outside_lines(&r, r["now_ms"].as_u64().unwrap_or(0)) {
                    println!("other pipeline: {l}");
                }
            }
            for a in s["active"].as_array().into_iter().flatten() {
                println!(
                    "active: #{} {} on {}",
                    a["task"],
                    a["step"].as_str().unwrap_or("?"),
                    a["display"].as_str().unwrap_or("?")
                );
            }
            println!(
                "projects: {}",
                s["projects"].as_array().map_or(String::new(), |p| p
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", "))
            );
            for (name, p) in s["pools"].as_object().into_iter().flatten() {
                println!(
                    "pool {name}: {} queued of {} ({} threads)",
                    p["queued"], p["queue_capacity"], p["threads"]
                );
            }
            let stalls = s["event_loop_stalls"].as_u64().unwrap_or(0);
            if stalls > 0 {
                println!(
                    "event loop stalls: {stalls} (worst {} ms)",
                    s["worst_stall_ms"]
                );
            }
            if s["bugs"].as_u64().unwrap_or(0) > 0 {
                println!("bugs filed by the daemon: {}", s["bugs"]);
            }
            if let Some(n) = s["thread_panics"]["count"].as_u64().filter(|n| *n > 0) {
                println!(
                    "THREAD PANICS: {n} (last: {})",
                    s["thread_panics"]["last"].as_str().unwrap_or("?")
                );
            }
            for p in s["definitions"].as_array().into_iter().flatten() {
                let hash = p["hash"].as_str().unwrap_or("");
                println!(
                    "definition for {}: {} ({})",
                    p["project"].as_str().unwrap_or("?"),
                    &hash[..12.min(hash.len())],
                    p["origin"].as_str().unwrap_or("?")
                );
                if let Some(errs) = p["refused"].as_array() {
                    println!("  the latest edit was REFUSED and the one above keeps running:");
                    errs.iter()
                        .filter_map(Value::as_str)
                        .for_each(|e| println!("    {e}"));
                }
            }
            let e = &s["exploring"];
            let skipped: u64 = [
                "skipped_by_flag",
                "skipped_by_clarity",
                "skipped_by_classifier",
            ]
            .iter()
            .map(|k| e[k].as_u64().unwrap_or(0))
            .sum();
            if skipped + e["explored"].as_u64().unwrap_or(0) > 0 {
                println!(
                    "exploring: {} explored, {} skipped (flag {}, clarity {}, classifier {}), {} sent back, {} explored anyway",
                    e["explored"], skipped, e["skipped_by_flag"], e["skipped_by_clarity"], e["skipped_by_classifier"], e["sent_back_to_exploring"], e["explore_anyway"]
                );
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
                    let explored = a.flag("--explored");
                    let after: Vec<u64> = a
                        .opt("--after")
                        .map(|v| {
                            v.split(',')
                                .map(|x| {
                                    x.trim()
                                        .trim_start_matches('#')
                                        .parse::<u64>()
                                        .unwrap_or_else(|_| {
                                            die(&format!(
                                                "--after takes task ids like 3,4, not {x:?}"
                                            ))
                                        })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
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
                    let r = or_die(d.call("POST", "/tasks", Some(&json!({"project": project, "title": title, "body": body, "priority": priority, "proposed": proposed, "explored": explored, "after": after}))));
                    if json_out {
                        return println!("{r:#}");
                    }
                    println!("Filed task #{} in {project}", r["task"]);
                }
                "list" => {
                    let state = a.opt("--state");
                    let pending = a.flag("--pending-issues");
                    finish(&a);
                    if pending {
                        let r = or_die(d.call("GET", "/pending-issues", None));
                        if json_out {
                            return println!("{r:#}");
                        }
                        let list = r["pending"].as_array().cloned().unwrap_or_default();
                        if list.is_empty() {
                            println!("no issues waiting to be imported");
                        }
                        for i in list {
                            println!(
                                "{} #{}  {}\n    {}",
                                i["project"].as_str().unwrap_or("?"),
                                i["number"],
                                i["title"].as_str().unwrap_or(""),
                                i["reason"].as_str().unwrap_or("")
                            );
                        }
                        return;
                    }
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
                    let watch = a.flag("--watch") || a.flag("-w");
                    finish(&a);
                    let t = or_die(d.call("GET", &format!("/tasks/{id}"), None));
                    if watch {
                        // What is running now, then every push, until the task is over.
                        if json_out {
                            println!("{}", t["progress"]);
                        } else {
                            println!("{}", progress::lines(&t["progress"]));
                        }
                        or_die(progress::watch(&d, Some(id), json_out).map_err(CallError::Io));
                        return;
                    }
                    if json_out {
                        return println!("{t:#}");
                    }
                    println!("#{} {}", t["id"], t["title"].as_str().unwrap_or(""));
                    println!("  project   {}", t["project"].as_str().unwrap_or("?"));
                    println!("  state     {}", t["state"].as_str().unwrap_or("?"));
                    println!("  activity  {}", t["activity"].as_str().unwrap_or("?"));
                    if let Some(m) = t["model"]["display"].as_str() {
                        println!("  model     {m}");
                        if let Some(w) = t["model"]["why"].as_str().filter(|w| !w.is_empty()) {
                            println!("            ({w})");
                        }
                    }
                    for r in t["steps"].as_array().into_iter().flatten() {
                        for l in r["launches"].as_array().into_iter().flatten() {
                            println!(
                                "  run {:<4} {:<9} {}",
                                r["run"],
                                l["kind"].as_str().unwrap_or(""),
                                l["display"].as_str().unwrap_or("")
                            );
                        }
                    }
                    if !t["progress"]["runs"].as_array().is_none_or(Vec::is_empty) {
                        for l in progress::lines(&t["progress"]).lines() {
                            println!("  progress  {l}");
                        }
                    }
                    let ids = |k: &str| {
                        t[k].as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(Value::as_u64)
                            .map(|n| format!("#{n}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    };
                    if let Some(l) = t["held_line"].as_str() {
                        println!("  {l}");
                    }
                    if let Some(r) = t["revive"].as_str() {
                        println!("  revive    {r}");
                    }
                    if !ids("blocked_by").is_empty() {
                        println!("  blocked by {}", ids("blocked_by"));
                        for l in t["held_by"].as_array().into_iter().flatten() {
                            println!(
                                "    #{} {} ({})",
                                l["task"],
                                l["state"].as_str().unwrap_or("?"),
                                l["origin"].as_str().unwrap_or("?")
                            );
                        }
                    }
                    if !ids("blocks").is_empty() {
                        println!("  blocks    {}", ids("blocks"));
                    }
                    for n in t["numbers"].as_array().into_iter().flatten() {
                        println!(
                            "  number    {} {}: {}{}{}",
                            n["kind"].as_str().unwrap_or("?"),
                            n["number"],
                            n["path"].as_str().unwrap_or(""),
                            if n["source"] == "stage" {
                                " (its stage)"
                            } else {
                                ""
                            },
                            if n["released"] == true {
                                " (released, not reused)"
                            } else {
                                ""
                            }
                        );
                    }
                    if let Some(w) = t["wait"].as_str() {
                        println!("  waiting   {w}");
                    }
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
                    if let Some(r) = t["route"].as_str() {
                        let why = match r {
                            "explore" => "explored first",
                            "flag" => "skipped exploring: filed as already explored",
                            "clarity" => "skipped exploring: the clarity check passed",
                            "classifier" => {
                                "skipped exploring: the project's classifier said it was clear"
                            }
                            "explore_anyway" => "explored, at the owner's request",
                            _ => r,
                        };
                        println!(
                            "  started   at {} ({why})",
                            t["steps"][0]["name"]
                                .as_str()
                                .filter(|s| !s.is_empty())
                                .unwrap_or("?")
                        );
                        if t["skipped_exploring"].as_bool().unwrap_or(false) {
                            println!(
                                "            `mason task explore-anyway {}` explores it after all",
                                t["id"]
                            );
                        }
                        if t["explore_returns"].as_u64().unwrap_or(0) > 0 {
                            println!("            the developing agent sent it back to exploring {} time(s), at no cost", t["explore_returns"]);
                        }
                    }
                    if let Some(f) = t["failure"].as_str() {
                        println!("  failed    {f}");
                        if let Some(to) = t["retry_to"].as_str() {
                            println!("  retry     goes to `{to}`");
                        }
                    }
                    for l in t["host_losses"].as_array().into_iter().flatten() {
                        println!(
                            "  host lost `{}` ({}): {}",
                            l["step"].as_str().unwrap_or("?"),
                            if l["rerun"].as_bool().unwrap_or(false) {
                                "re-run once on a fresh host"
                            } else {
                                "not re-run"
                            },
                            l["detail"].as_str().unwrap_or("")
                        );
                    }
                    if let Some(c) = t["closing"]["note"].as_str() {
                        println!("  tests     {c}");
                    }
                    for x in t["debts"].as_array().into_iter().flatten() {
                        println!(
                            "  debt #{:<3} {} {}{}",
                            x["id"],
                            x["scope"].as_str().unwrap_or("?"),
                            x["status"].as_str().unwrap_or("?"),
                            if x["deferred"].as_bool().unwrap_or(false) {
                                " (runs after landing)"
                            } else {
                                ""
                            }
                        );
                    }
                    if let Some(c) = t["card"].as_object().filter(|c| !c.is_empty()) {
                        let get = |k: &str| c.get(k).and_then(Value::as_str).unwrap_or("");
                        println!("\n  QUESTION  {}", get("ask"));
                        if !get("why").is_empty() {
                            println!("  why you   {}", get("why"));
                        }
                        for (i, o) in c
                            .get("options")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                            .enumerate()
                        {
                            println!(
                                "  {}. {}{}",
                                i + 1,
                                o["label"].as_str().unwrap_or(""),
                                if o["recommended"].as_bool().unwrap_or(false) {
                                    "   (recommended)"
                                } else {
                                    ""
                                }
                            );
                        }
                        println!("  other     any other text");
                        for l in c
                            .get("evidence")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                        {
                            println!("    | {}", l.as_str().unwrap_or(""));
                        }
                        if get("origin") != "agent" {
                            println!(
                                "  (this card was {} because the agent's did not fit: {})",
                                get("origin"),
                                c.get("problems")
                                    .and_then(Value::as_array)
                                    .map(|p| p
                                        .iter()
                                        .filter_map(Value::as_str)
                                        .collect::<Vec<_>>()
                                        .join("; "))
                                    .unwrap_or_default()
                            );
                        }
                        println!(
                            "  card      {}   (answer with `mason task answer {} <option>`)",
                            get("hash"),
                            t["id"]
                        );
                    }
                    if let Some(b) = t["body"].as_str().filter(|b| !b.trim().is_empty()) {
                        println!("\n{}", b.trim());
                    }
                    println!("\nsteps:");
                    for s in t["steps"].as_array().into_iter().flatten() {
                        println!(
                            "  run {:<3} {:<11} {:<12} attempt {}  {}",
                            s["run"],
                            s["step"].as_str().unwrap_or("?"),
                            s["name"].as_str().unwrap_or(""),
                            s["attempt"],
                            s["outcome"].as_str().unwrap_or("running")
                        );
                    }
                }
                "progress" => {
                    let id = task_id(&mut a);
                    let watch = a.flag("--watch") || a.flag("-w");
                    finish(&a);
                    let mut last = String::new();
                    loop {
                        let r = or_die(d.call("GET", &format!("/tasks/{id}/progress"), None));
                        let text = if json_out {
                            format!("{r:#}")
                        } else {
                            progress::lines(&r)
                        };
                        // A watch prints a line only when something moved.
                        if !watch || text != last {
                            println!("{text}");
                            last = text;
                        }
                        if !watch {
                            break;
                        }
                        let t = or_die(d.call("GET", &format!("/tasks/{id}"), None));
                        if matches!(t["state"].as_str(), Some("landed" | "failed" | "canceled")) {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(1000));
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
                    let from = a.opt("--from");
                    finish(&a);
                    // Without --from the daemon picks: the failed step when the
                    // failure was not about the code, otherwise developing. Say
                    // which before sending.
                    if from.is_none() {
                        let t = or_die(d.call("GET", &format!("/tasks/{id}"), None));
                        if let Some(to) = t["retry_to"].as_str() {
                            println!("Task #{id} will retry from `{to}`");
                        }
                    }
                    let body = from.as_ref().map_or(json!({}), |f| json!({"to": f}));
                    let r = or_die(d.call("POST", &format!("/tasks/{id}/retry"), Some(&body)));
                    println!(
                        "Retrying task #{id} from {}",
                        r["to"].as_str().unwrap_or("developing")
                    );
                }
                "skip" => {
                    let id = task_id(&mut a);
                    let suite = a.opt("--suite");
                    let step = a.opt("--step");
                    let reason = a.opt("--reason").unwrap_or_default();
                    std::env::set_var("MASON_REASON", &reason);
                    let owner = a
                        .opt("--owner")
                        .or_else(|| std::env::var("USER").ok())
                        .unwrap_or_default();
                    let deferred = a.flag("--deferred");
                    finish(&a);
                    let mut body = json!({"reason": reason, "owner": owner, "deferred": deferred});
                    match (suite, step) {
                        (Some(s), None) => body["suite"] = json!(s),
                        (None, Some(s)) => body["step"] = json!(s),
                        _ => die("name one suite or one step: --suite S or --step S"),
                    }
                    let r = or_die(d.call("POST", &format!("/tasks/{id}/skip"), Some(&body)));
                    println!(
                        "Skipped by force. Debt #{} is open{}; its closing verdict is downgraded.",
                        r["debt"],
                        if deferred {
                            " and will run after landing"
                        } else {
                            ""
                        }
                    );
                }
                "answer" => {
                    let id = task_id(&mut a);
                    let who = a
                        .opt("--as")
                        .or_else(|| std::env::var("MASON_AS").ok())
                        .or_else(|| std::env::var("USER").ok())
                        .unwrap_or_default();
                    let card = a.opt("--card");
                    let answer: Vec<String> = std::mem::take(&mut a.rest);
                    if answer.is_empty() {
                        die("give the answer: an option number, or text for \"Other\"");
                    }
                    // Without --card, answer the card as it stands now.
                    let hash = match card {
                        Some(h) => h,
                        None => {
                            let t = or_die(d.call("GET", &format!("/tasks/{id}"), None));
                            t["card"]["hash"].as_str().unwrap_or_default().to_string()
                        }
                    };
                    let r = or_die(d.call(
                        "POST",
                        &format!("/tasks/{id}/answer"),
                        Some(&json!({"answer": answer.join(" "), "hash": hash, "as": who})),
                    ));
                    println!(
                        "Answered task #{id}: {}",
                        r["answer"].as_str().unwrap_or("")
                    );
                }
                "land-now" | "force" => {
                    let id = task_id(&mut a);
                    let reason = a.opt("--reason").unwrap_or_default();
                    std::env::set_var("MASON_REASON", &reason);
                    finish(&a);
                    let r = or_die(d.call(
                        "POST",
                        &format!("/tasks/{id}/{sub}"),
                        Some(&json!({"reason": reason})),
                    ));
                    println!(
                        "Debt #{} recorded. {}",
                        r["debt"],
                        r["note"].as_str().unwrap_or("")
                    );
                }
                "refresh" => {
                    let id = task_id(&mut a);
                    finish(&a);
                    let r =
                        or_die(d.call("POST", &format!("/tasks/{id}/refresh"), Some(&json!({}))));
                    if r["tracker"] == false {
                        println!(
                            "{}",
                            r["note"].as_str().unwrap_or("the project has no tracker")
                        );
                        if let Some(c) = r["config"].as_str() {
                            println!("config: {c}");
                        }
                    } else if let Some(e) = r["error"].as_str() {
                        println!("the tracker did not answer: {e}");
                    } else {
                        println!(
                            "asked the tracker: {} imported, {} updated, {} closed, {} ignored, {} unchanged, {} waiting for a slot",
                            r["imported"], r["updated"], r["closed"], r["ignored"], r["unchanged"], r["waiting_slot"]
                        );
                    }
                }
                "open" => {
                    let id = task_id(&mut a);
                    let anyway = a.flag("--anyway");
                    finish(&a);
                    let s = or_die(d.call("GET", &format!("/tasks/{id}/session"), None));
                    if !s["idle"].as_bool().unwrap_or(true) && !anyway {
                        die("the step is running; a second session would collide with it. Wait for it to be idle, or pass --anyway");
                    }
                    let bin = s["claude"].as_str().unwrap_or("claude").to_string();
                    let wt = s["worktree"].as_str().unwrap_or(".").to_string();
                    let mut cmd = std::process::Command::new(&bin);
                    cmd.current_dir(&wt);
                    match s["session"].as_str() {
                        Some(sid) => {
                            println!("Resuming the agent's session {sid} in {wt}");
                            cmd.args(["--resume", sid]);
                        }
                        None => println!("No session to resume; starting a fresh one in {wt}"),
                    }
                    match cmd.status() {
                        Ok(st) => exit(st.code().unwrap_or(1)),
                        Err(e) => die(&format!("cannot run `{bin}`: {e}")),
                    }
                }
                "approve" => {
                    let id = task_id(&mut a);
                    finish(&a);
                    // A change to MASON's own rules is approved by the owner's identity: this
                    // user's, unless `--as` said otherwise.
                    if std::env::var("MASON_AS").is_err() {
                        if let Ok(u) = std::env::var("USER") {
                            std::env::set_var("MASON_AS", u);
                        }
                    }
                    or_die(d.call("POST", &format!("/tasks/{id}/approve"), None));
                    println!("Approved task #{id}");
                }
                "unblock" | "block" | "revive" => {
                    let id = task_id(&mut a);
                    let list = |v: String, flag: &str| -> Vec<u64> {
                        v.split(',')
                            .map(|x| {
                                x.trim()
                                    .trim_start_matches('#')
                                    .parse::<u64>()
                                    .unwrap_or_else(|_| {
                                        die(&format!("{flag} takes task ids like 3,4, not {x:?}"))
                                    })
                            })
                            .collect()
                    };
                    let body = match sub.as_str() {
                        "unblock" => {
                            let from = a.opt("--from").map(|v| list(v, "--from"));
                            let all = a.flag("--all");
                            match (from, all) {
                                (Some(_), true) => die("use --from or --all, not both"),
                                (Some(f), false) => Some(json!({"from": f})),
                                (None, true) => Some(json!({"all": true})),
                                (None, false) => die("which blockers? --from ID[,ID...] or --all"),
                            }
                        }
                        "block" => {
                            let after = a
                                .opt("--after")
                                .map(|v| list(v, "--after"))
                                .unwrap_or_else(|| {
                                    die("block after which tasks? --after ID[,ID...]")
                                });
                            Some(json!({"after": after}))
                        }
                        _ => None,
                    };
                    finish(&a);
                    // The owner's identity, as `approve` takes it: this user's, unless MASON_AS says otherwise.
                    if std::env::var("MASON_AS").is_err() {
                        if let Ok(u) = std::env::var("USER") {
                            std::env::set_var("MASON_AS", u);
                        }
                    }
                    let r = or_die(d.call("POST", &format!("/tasks/{id}/{sub}"), body.as_ref()));
                    println!("{}", r["message"].as_str().unwrap_or("done"));
                }
                "relink" => {
                    let id = task_id(&mut a);
                    let issue: u64 = a
                        .opt("--issue")
                        .and_then(|v| v.trim_start_matches('#').parse().ok())
                        .unwrap_or_else(|| die("which issue? --issue N"));
                    finish(&a);
                    if std::env::var("MASON_AS").is_err() {
                        if let Ok(u) = std::env::var("USER") {
                            std::env::set_var("MASON_AS", u);
                        }
                    }
                    or_die(d.call(
                        "POST",
                        &format!("/tasks/{id}/relink"),
                        Some(&json!({"issue": issue})),
                    ));
                    println!("Task #{id} is now linked to issue #{issue}");
                }
                "reject" => {
                    let id = task_id(&mut a);
                    let reason = a
                        .opt("--reason")
                        .unwrap_or_else(|| "no reason given".into());
                    std::env::set_var("MASON_REASON", &reason);
                    finish(&a);
                    or_die(d.call(
                        "POST",
                        &format!("/tasks/{id}/reject"),
                        Some(&json!({"reason": reason})),
                    ));
                    println!("Rejected task #{id}");
                }
                "explore-anyway" => {
                    let id = task_id(&mut a);
                    finish(&a);
                    let r = or_die(d.call("POST", &format!("/tasks/{id}/explore-anyway"), None));
                    println!(
                        "Task #{id} will explore (step {})",
                        r["step"].as_str().unwrap_or("?")
                    );
                }
                "override" => {
                    let id = task_id(&mut a);
                    let key = a
                        .pos()
                        .unwrap_or_else(|| die("which setting? e.g. step.develop.model"));
                    let value = a.pos().unwrap_or_else(|| die("which value?"));
                    finish(&a);
                    or_die(d.call("POST", &format!("/tasks/{id}/overrides"), Some(&json!({"key": key, "value": value, "owner": std::env::var("USER").unwrap_or_else(|_| "owner".into())}))));
                    println!("Task #{id}: {key} = {value}");
                }
                other => die(&format!("unknown task command {other:?}\n\n{HELP}")),
            }
        }
        other => die(&format!("unknown command {other:?}\n\n{HELP}")),
    }
}

fn every_text(ms: u64) -> String {
    match ms {
        m if m >= 86_400_000 && m % 86_400_000 == 0 => format!("{}d", m / 86_400_000),
        m if m >= 3_600_000 && m % 3_600_000 == 0 => format!("{}h", m / 3_600_000),
        m if m >= 60_000 && m % 60_000 == 0 => format!("{}m", m / 60_000),
        m if m % 1000 == 0 => format!("{}s", m / 1000),
        m => format!("{m}ms"),
    }
}

fn ms_text(ms: u64) -> String {
    match ms {
        m if m >= 3_600_000 => format!("{:.1}h", m as f64 / 3_600_000.0),
        m if m >= 60_000 => format!("{:.1}m", m as f64 / 60_000.0),
        m => format!("{:.1}s", m as f64 / 1000.0),
    }
}

fn print_health(r: &serde_json::Value) {
    println!("Findings and what came of their fixes");
    let list = r["findings"].as_array().cloned().unwrap_or_default();
    if list.is_empty() {
        println!("  none yet");
    }
    for f in list {
        println!(
            "  #{:<3} [{}] {} (proposal #{}, metric {})",
            f["id"],
            f["verdict"].as_str().unwrap_or("?"),
            f["title"].as_str().unwrap_or(""),
            f["task"],
            f["metric"].as_str().unwrap_or("?")
        );
        if let Some(t) = f["text"].as_str().filter(|t| !t.is_empty()) {
            println!("        {t}");
        }
    }
    println!("Recurring jobs");
    for s in r["schedules"].as_array().cloned().unwrap_or_default() {
        println!(
            "  {:<28} every {:<6} runs {:<4} {}",
            s["name"].as_str().unwrap_or("?"),
            every_text(s["every_ms"].as_u64().unwrap_or(0)),
            s["runs"],
            s["last_status"].as_str().unwrap_or("")
        );
    }
    let q = r["triage"]["queue"].as_array().map_or(0, Vec::len);
    println!(
        "Triage: {} ({} waiting for your review, {} reviewed)",
        if r["triage"]["enabled"].as_bool().unwrap_or(false) {
            "on"
        } else {
            "off"
        },
        q,
        r["triage"]["reviewed"]
    );
}

fn print_digest(r: &serde_json::Value) {
    let n = |k: &str| r[k].to_string();
    println!("Landed: {}", n("landed"));
    match r["median_claim_to_land_ms"].as_u64() {
        Some(m) => println!(
            "Median time from claim to land: {} ({} task(s))",
            ms_text(m),
            n("claim_to_land_samples")
        ),
        None => println!("Median time from claim to land: no landed task in the window"),
    }
    match r["interventions_per_landed"].as_f64() {
        Some(v) => println!(
            "Interventions per landed task: {v:.2} ({} interventions)",
            n("interventions")
        ),
        None => println!(
            "Interventions per landed task: n/a ({} interventions, none landed)",
            n("interventions")
        ),
    }
    println!("Tokens per landed task: not recorded");
    for (title, key) in [
        ("Time by state", "time_by_state_ms"),
        ("Waiting, by reason", "wait_by_reason_ms"),
    ] {
        println!("{title}:");
        if let Some(m) = r[key].as_object() {
            for (k, v) in m {
                println!("  {k:<20} {}", ms_text(v.as_u64().unwrap_or(0)));
            }
        }
    }
    let ex = &r["exploring"];
    println!(
        "Exploring skipped for {} task(s); {} came back to exploring",
        ex["skipped"], ex["came_back"]
    );
    if let Some(m) = ex["by_route"].as_object() {
        for (route, v) in m {
            println!(
                "  {route:<16} {} task(s), {} came back",
                v["tasks"], v["came_back"]
            );
        }
    }
    println!("Forced landings:");
    let forced = r["forced_landings"].as_array().cloned().unwrap_or_default();
    if forced.is_empty() {
        println!("  none");
    }
    for f in forced {
        println!(
            "  task #{} ({}, {}): {}",
            f["task"],
            f["project"].as_str().unwrap_or("?"),
            f["kind"].as_str().unwrap_or("?"),
            f["reason"].as_str().unwrap_or("")
        );
    }
    println!("Flaky: {}", r["flaky"].as_object().map_or(0, |m| m.len()));
    println!(
        "Debts outstanding: {}",
        r["debts_outstanding"].as_array().map_or(0, Vec::len)
    );
    println!("Fixes:");
    let fs = r["findings"].as_array().cloned().unwrap_or_default();
    if fs.is_empty() {
        println!("  none");
    }
    for f in fs {
        println!(
            "  #{} {} [{}] {}",
            f["id"],
            f["title"].as_str().unwrap_or(""),
            f["verdict"].as_str().unwrap_or("?"),
            f["text"].as_str().unwrap_or("")
        );
    }
}

fn tokens_text(t: &Value, out: &mut String) {
    for (title, key) in [
        ("by project", "byProject"),
        ("by model", "byModel"),
        ("top tasks", "topTasks"),
    ] {
        let lines = t[key].as_array().cloned().unwrap_or_default();
        if lines.is_empty() {
            continue;
        }
        out.push_str(&format!("    {title}:\n"));
        for l in lines {
            let n = |k: &str| {
                l[k].as_str()
                    .and_then(|s| s.parse::<u64>().ok())
                    .or(l[k].as_u64())
            };
            let mut parts = vec![];
            for (label, k) in [
                ("in", "input"),
                ("out", "output"),
                ("cache read", "cacheRead"),
                ("cache write", "cacheWrite"),
            ] {
                if let Some(v) =
                    n(k).filter(|_| l[format!("has{}", pascal(k))].as_bool() == Some(true))
                {
                    parts.push(format!("{label} {}", mason_core::usage_text::count(v)));
                }
            }
            if l["hasCost"].as_bool() == Some(true) {
                parts.push(format!("${:.2}", l["costUsd"].as_f64().unwrap_or(0.0)));
            }
            out.push_str(&format!(
                "      {}: {} ({})\n",
                l["name"].as_str().unwrap_or("?"),
                if parts.is_empty() {
                    "nothing reported".to_string()
                } else {
                    parts.join(", ")
                },
                l["age"].as_str().unwrap_or("")
            ));
        }
    }
}

fn pascal(k: &str) -> String {
    k.split('_')
        .map(|w| {
            let mut c = w.chars();
            c.next().map_or(String::new(), |f| {
                f.to_uppercase().collect::<String>() + c.as_str()
            })
        })
        .collect()
}

/// The usage view as text, from its JSON.
fn usage_text(r: &Value) -> String {
    let mut out = String::new();
    for p in r["providers"].as_array().into_iter().flatten() {
        let g = |k: &str| p[k].as_str().unwrap_or("");
        out.push_str(&format!(
            "{} ({}, {}): {} of {} sessions{}\n",
            g("name"),
            g("adapter"),
            g("billing"),
            p["sessionsUsed"].as_u64().unwrap_or(0),
            p["sessionsLimit"].as_u64().unwrap_or(0),
            if p["hasSnapshot"].as_bool() == Some(true) {
                format!(", {}", g("age"))
            } else {
                ", nothing reported yet".into()
            }
        ));
        for w in p["windows"].as_array().into_iter().flatten() {
            let line = if w["reported"].as_bool() == Some(true) {
                let pct = if w["hasPercent"].as_bool() == Some(true) {
                    format!("{}%", w["percent"].as_f64().unwrap_or(0.0).round())
                } else {
                    "no percentage reported".into()
                };
                format!(
                    "{pct}, {}, {}{}",
                    w["state"].as_str().unwrap_or("ok"),
                    w["resetsText"]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .unwrap_or("no reset reported"),
                    w["trend"]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .map_or(String::new(), |t| format!(" ({t})"))
                )
            } else {
                "not reported yet".into()
            };
            out.push_str(&format!(
                "  {}: {line}\n",
                w["label"].as_str().unwrap_or("?")
            ));
        }
        // Index the Value, not its Map: a field left out (proto JSON drops false and zero)
        // reads as null instead of panicking.
        if let Some(s) = Some(&p["spend"]).filter(|s| s["hasSpend"].as_bool() == Some(true)) {
            let f = |k: &str| s[k].as_f64().unwrap_or(0.0);
            let budget = if s["hasBudget"].as_bool() == Some(true) {
                format!(" of ${:.2} monthly budget", f("budgetUsd"))
            } else {
                String::new()
            };
            out.push_str(&format!(
                "  spend: ${:.2} today, ${:.2} this week, ${:.2} this month{budget} ({})\n",
                f("todayUsd"),
                f("weekUsd"),
                f("monthUsd"),
                s["age"].as_str().unwrap_or("")
            ));
        } else if g("billing") == "api" {
            out.push_str("  spend: nothing reported yet\n");
        }
        for k in ["overageText", "blockedText"] {
            if !g(k).is_empty() {
                out.push_str(&format!("  {}\n", g(k)));
            }
        }
        for (key, fallback) in [("tokensWindow", ""), ("tokensWeek", "")] {
            let t = &p[key];
            let has = ["byProject", "byModel", "topTasks"]
                .iter()
                .any(|k| t[*k].as_array().is_some_and(|a| !a.is_empty()));
            if has {
                out.push_str(&format!(
                    "  tokens, {}:{fallback}\n",
                    t["period"].as_str().unwrap_or("")
                ));
                tokens_text(t, &mut out);
            }
        }
        out.push_str(&format!("  doing: {}\n", g("doing")));
    }
    if out.is_empty() {
        out.push_str("no provider accounts\n");
    }
    out
}

// SPDX-License-Identifier: GPL-3.0-or-later

//! `mason`: the command-line client.
//!
//! ```text
//! mason task add <title> [--project P] [--body TEXT | --body-file F | --body -] [--priority urgent|high|normal|low] [--proposed]
//! mason task edit <id> [--title T] [--body TEXT | --body-file F | --body -] [--reason R]
//! mason task list [--state S] [--json]
//! mason task show <id> [--json] [--watch]
//! mason task cancel <id> [--delete-branch]
//! mason task retry <id> [--from exploring|developing|testing|review|landing]
//! mason logs <id> [--follow]
//! mason task skip <id> (--suite S | --step S | --all) [--reason R] [--deferred]
//! mason debt list | run <id>
//! mason quarantine list | add <suite> <test> --owner O --days N [--reason R] | remove <suite> <test>
//! mason test parse <parser> <file> [--min N]
//! mason task answer <id> <option number | 1,3 for a multi-select card | text> [--context TEXT] [--card HASH] [--as NAME]
//! mason task land-now <id> | force <id> [--reason R]
//! mason task open <id> [--anyway]
//! mason pause [--project P] [--mode all|claims|landing] --for 2h | mason resume [--project P] [--mode M] | mason pause list
//! mason ssh-key list | use <path> [--project P]
//! mason alerts [--all] | mason metrics [--days N]
//! mason health | mason digest [--days N] [--project P] | mason schedule list | run <name>
//! mason triage list | review <id> (--agree | --disagree)
//! mason status [--json] [--watch]
//! mason hosts [--json]
//! mason run --needs compile[=N],... [--exclusive] [--label L] [--origin O] -- <cmd>...
//! mason hold --needs R[,R...] [--exclusive] --pipeline NAME --label L [--pid P]
//! mason targets list [--json]
//! mason cpu [--json] | mason resources [--json] | mason resources limit <resource> <units>
//! mason resources gate <name> <value> | mason resources gate <name> --reset
//! mason remote on | off | status [--json]
//! mason doctor
//! mason init [--yes] [--tracker T] [--agent A] [--sessions N] [--workflow W] [--suites accept|skip] [--base B] [--landing L] [--alerts LIST|none] [--no-prove]
//! mason adopt | mason remove [--yes]
//! mason remote setup --project P [--runs-on L]
//! ```
//! Global: `--data-dir D`, `--config-dir C` (or `MASON_DATA_DIR`, `MASON_CONFIG_DIR`).

mod adopt;
mod batch_ci;
mod client;
mod definition;
mod doctor;
mod guard;
mod hosts;
mod init;
mod node;
mod progress;
mod remote;
mod remove;
mod resources;
mod service;
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
  mason task edit <id> [--title T] [--body TEXT | --body-file F | --body -] [--reason R]
                         rewrite a started or failed task's text (owner only); the next step
                         reads it. Edit when the scope changed, a retry note when a decision
                         applies to one round
  mason task list [--state S] [--json]
  mason task list --pending-issues    open issues the tracker has not imported, with why
  mason task relink <id> --issue N    tie a live task to issue N whose task was canceled (owner only)
  mason task unblock <id> [--from ID[,ID...] | --all]   release a queued task from some or all of its blockers (owner only)
  mason task block <id> --after ID[,ID...]   add blockers to a task that has not started (owner only)
  mason task revive <id>              put a canceled task whose branch was kept back in the queue (owner only)
  mason task show <id> [--json] [--watch]   with --watch, follow the live step progress
  mason task cancel <id> [--delete-branch]
  mason task retry <id> [--from STEP] [--restart] [--message TEXT]
                                      the message is shown once in the next round's prompt
  mason task diagnose <id>            ask for a diagnosis of a failed task (owner only)
  mason task diagnosis <id> [apply|dismiss N]   what the diagnosis agent said about a failed task;
                                      apply (its recommended option) or dismiss a pending one
  mason task approve <id> [--priority P]   let a proposed or deferred task start (queued at P: urgent,
                                      high, normal or low), or an approval step go on
  mason task defer <id>               put a proposed task aside (state deferred); it comes back only
                                      by hand: approve, reject or undefer
  mason task undefer <id>             move a deferred task back to proposed
  mason task reject <id> [--reason R] reject a proposed task or an approval step
  mason task explore-anyway <id>      explore after all, even if it skipped exploring
  mason task override <id> <key> <value>
  mason task priority <id> urgent|high|normal|low   owner only; re-ranks its waiting requests
  mason task update-priority <id> urgent|normal   owner only; urgent: its landing takes the daemon's own update over at once
  mason task answer <id> <option number | 1,3 | text> [--context TEXT] [--card HASH] [--as NAME]
                                      answer the open question or a failed task's diagnosis card (a multi-select card takes 1,3);
                                      --context adds your words to the answer; the card's hash guards against a stale one
  mason task refresh <id>             ask the tracker now (what opening a task does)
  mason task land-now <id> | force <id> [--reason R]   marks the task and records an override and a debt
  mason task open <id> [--anyway]     open an interactive agent in the task's worktree, resuming its session
  mason session list | open <task> [--confirm] | show <id> [--after N] | send <id> <text|-> [--no-wait] | end <id>
                                      the owner's chat sessions with an agent (what the monitor's dialog does)
  mason health pipeline [--json] | health check | health history [--limit N]
                                      the pipeline's own health check: what looks wrong now, run a check (it may start a health
                                      session), and what self-correction did and why (plain `mason health` is the report below)
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
  mason report context [--days N] [--project P]
                                      send-back rate per step by how full the agent's context got
  mason report [--days N] [--project P]   throughput, send-backs, waits, interventions, host losses, flakes, debts
  mason digest [--days N] [--project P]   landed, interventions, time by state, skips, forced landings (recorded data only)
  mason schedule list | run <name> [--force]    recurring jobs (introspect, analyst:<project>, <project>/<schedule>); run one now (a full test follows the record unless --force)
  mason triage list | review <id> (--agree | --disagree)   the triage agent's sampled suggestions; it never answers
  mason task skip <id> (--suite S | --step S | --all) [--reason R] [--deferred]
                                      skip a suite or step by force, even mid-run; records a debt;
                                      --all is a fast track: the task skips the suites, but run_check
                                      and the landing floor still build the change and run the tests it
                                      adds (new #[test] fns, new exit scripts); a red there still stops it
  mason debt list | run <id>          what was skipped and not yet proven; run one on the base branch
  mason quarantine list | add <suite> <test> --owner O --days N [--reason R] | remove <suite> <test>
  mason test parse <parser> <file> [--min N]   read a results file the way a suite would, and judge it
  mason logs <id> [--follow]
  mason status [--json] [--watch]     with --watch, follow every running step's progress
  mason hosts [--json]                this Mac, each node and GitHub, with what runs on each now
  mason service install [--print] [--force] | uninstall | status
                                      run masond --managed under launchd (macOS) or a systemd user unit
  mason doctor                        checks the setup, and that `mason` on PATH is this program
  mason init [--yes] [--workflow solo-fast|reviewed-pr|batched-ci|device-mobile|human-gated]
             [--tracker local|github] [--agent claude|codex] [--agent-bin P] [--sessions N]
             [--suites accept|skip] [--base B] [--landing direct|direct-ci|pr|batched]
             [--alerts sound,speech,email,push|none] [--ci-workflow yes|no] [--id ID]
             [--land no|merge|push|pr] [--no-prove] [--force]
                                      guided setup: writes .mason/ and the Filing notes on branch
                                      mason/setup and the project file; lands it only on --land
                                      (default no; never pushes otherwise), then registers the
                                      project with a running masond, files the proof task and
                                      opens the monitor; every question has a flag
  mason init --prove-only             file the one harmless first task, once masond knows the project
  mason adopt [--id ID] [--base B] [--test CMD]   register a project that already has .mason/ (never writes it)
  mason remote setup --project P [--runs-on L]   write .github/workflows/mason-run.yml on a mason/setup-remote branch (nothing pushed)
  mason remove [--yes] [--id ID]      delete the project file, .mason/ and the Filing notes (lists first without --yes)
  mason run --needs compile[=N],... [--exclusive] [--label L] [--origin O] -- <cmd>...
                                      queue for the pipeline's resources, ahead of its work
  mason run ... --pipeline NAME [--priority urgent|high|normal|low] [--progress 0-6]
                                      the same for another pipeline's work: ranked with MASON's tasks, behind a person's
  mason hold --needs R[,R...] [--exclusive] --pipeline NAME --label L [--pid P]
                                      hold resources for a process's life: prints `granted`, then holds until
                                      stdin closes, --pid exits or it is signalled
  mason resources [--json]            limits, who holds what, and every wait explained
  mason cpu [--json]                  where the machine's CPU goes, by task (share of all cores)
  mason targets list [--json]         simulators, emulators and phones, and who has each
  mason resources limit <resource> <units>   change a limit while things run (llm:claude, compile, ...)
  mason resources gate <name> <value>   change a compile gate while masond runs, kept across restarts (compile_cpu_max,
                                      compile_settle_s, compile_jobs, compile_idle_kill_s, syspolicyd_max_pct, thermal_min_pct, thermal_max_c, stall_ms,
                                      memory.floor_mb, disk.floor_gb)
  mason resources gate <name> --reset   go back to resources.toml, or the default
  mason remote mode <mode> [--project P]  placement mode: local, remote, favor-local or favor-remote, for all of MASON
                                      or one project; a task's own is `mason task run-on <id> <mode>|default`
  mason remote on|off|status [--json]  aliases for mode remote / local, and the mode per project and where it came from. Send every compile and test that can go to GitHub Actions there, or bring them back
                                      (suites with remote = false, a device or exclusive always stay here)
  mason hook bash [--needs compile]   optional Claude Code PreToolUse hook: builds go through the queue, and in a
                                      MASON agent session destructive git and rm commands are refused
  mason hook guard                    that refusal alone; MASON registers it for every Claude agent it starts
  mason hook skill --policy <file>    refuse a skill that is not the project's or the owner's; registered the same way
  mason definition check [FILE] [--project P]
                                      validate a definition (default .mason/mason.toml) and dry-run it;
                                      --project also knows P's approved plugin step types
  mason definition dry-run [FILE] [--scenarios FILE]
  mason ssh-key list | use <path> [--project P]   the SSH key a project's pushes use; choose the right one after a push was refused
  mason editor show|op|undo|redo|discard|simulate|apply [--project P]   the pipeline editor, without a screen
  mason definition show [--project P] | reload [--project P]
  mason project reload [--project P]      re-read ~/.config/mason/projects/P.toml, no restart
  mason project rename OLD NEW [--yes]    rename a project and keep its history (lists first without --yes)
  mason extension list|show|approve [NAME] [--project P]   project extensions; approving is the owner's
  mason node pair-code | accept on|off | status | doctor | jobs | stop-jobs | coordinators [remove NAME]
                                      this Mac as a node, taking compile and test runs from a coordinator
  mason node list | add [NAME | --host H:PORT --fp F] [--code C] | remove NAME | use on|off
  mason node update NAME | --all      send a node the runtime this coordinator runs, now (`mason update` does it for every node)
  mason node export-runtime --arch arm64|x86_64 --out DIR   the signed runtime and MASON Daemon.app for a node
                                      the coordinator's nodes: found, paired, whether each takes work
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
  mason update [--channel stable|main|release|branch:X | --sha S] [--rollback] [--check] [--now] [--auto]
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

/// Run the session command the daemon gave (`argv`) in the task's worktree and return its exit
/// code. An older daemon gives no `argv`; then it is Claude's, as before.
fn run_session(s: &Value) -> i32 {
    let wt = s["worktree"].as_str().unwrap_or(".").to_string();
    let provider = s["provider"].as_str().unwrap_or("claude").to_string();
    let sid = s["session"].as_str();
    let argv: Vec<String> = match s["argv"].as_array() {
        Some(a) if !a.is_empty() => a
            .iter()
            .filter_map(|x| x.as_str().map(String::from))
            .collect(),
        _ => {
            let mut v = vec![s["claude"].as_str().unwrap_or("claude").to_string()];
            if let Some(i) = sid {
                v.extend(["--resume".to_string(), i.to_string()]);
            }
            v
        }
    };
    match sid {
        Some(i) => println!("Resuming the {provider} session {i} in {wt}"),
        None => println!("No session to resume; starting a fresh {provider} one in {wt}"),
    }
    let mut cmd = std::process::Command::new(&argv[0]);
    cmd.args(&argv[1..]).current_dir(&wt);
    match cmd.status() {
        Ok(st) => {
            let code = st.code().unwrap_or(1);
            if code != 0 && sid.is_some() {
                eprintln!(
                    "`{}` ended with status {code}. If it said the session was not found, it was \
                     recorded under another working directory or config dir than this shell's: \
                     the agent ran in {wt}, and {provider} keeps sessions under its own config \
                     dir (for Claude `~/.claude`, or $CLAUDE_CONFIG_DIR).",
                    argv.join(" ")
                );
            }
            code
        }
        Err(e) => {
            eprintln!("cannot run `{}`: {e}", argv[0]);
            127
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

/// The words after `mason task answer <id>` as one answer: `1,3` stays as
/// typed, and `1 3` arrives as two words and is joined with a space.
fn answer_text(words: &[String]) -> String {
    words.join(" ")
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

/// `HH:MM` local time of a time in ms since the epoch (UTC if `date` cannot say).
fn local_clock(ms: u64) -> String {
    let at = ms / 1000;
    std::process::Command::new("date")
        .args(["-r", &at.to_string(), "+%H:%M"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| format!("{:02}:{:02} UTC", at / 3600 % 24, at / 60 % 60))
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

/// How long from `now` until `ms` (both ms since the epoch), rounded up: to the minute
/// below 48 hours (`in 2h 59m`), else to the day.
fn ago_until(ms: u64, now: u64) -> String {
    let mins = ms.saturating_sub(now).div_ceil(60_000);
    match (mins / 60, mins % 60) {
        (0, m) => format!("in {m}m"),
        (h @ 1..=47, 0) => format!("in {h}h"),
        (h @ 1..=47, m) => format!("in {h}h {m}m"),
        _ => format!("in {}d", mins.div_ceil(24 * 60)),
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
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
    if a.rest.first().map(String::as_str) == Some("--version") {
        println!(
            "{}",
            mason_daemon::version::line(env!("CARGO_PKG_VERSION"), env!("MASON_COMMIT"))
        );
        return;
    }
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
        let policy = a.opt("--policy");
        finish(&a);
        if sub != "bash" && sub != "guard" && sub != "skill" {
            die("the hooks are `mason hook bash`, `mason hook guard` and `mason hook skill`");
        }
        let mut input = String::new();
        let _ = std::io::stdin().read_to_string(&mut input);
        if sub == "skill" {
            // An error exits non-zero, and the hook's command then refuses the call.
            let policy = policy.unwrap_or_else(|| die("`mason hook skill` needs --policy <file>"));
            let env = |k: &str| std::env::var(k).ok();
            match guard::hook_skill(&input, std::path::Path::new(&policy), &env) {
                Ok(Some(out)) => println!("{out}"),
                Ok(None) => {}
                Err(e) => die(&e),
            }
            return;
        }
        let env = |k: &str| std::env::var(k).ok();
        // `hook bash` is also in a repository's own settings, which a person's session
        // reads: it refuses only inside a session MASON started (s1107).
        if sub == "guard" || env("MASON_STEP_KIND").is_some() || env("MASON_TASK").is_some() {
            if let Some(out) = guard::hook_guard(&input, &env) {
                println!("{out}");
                return;
            }
        }
        if sub == "guard" {
            return;
        }
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
    if cmd == "service" {
        exit(service::run(
            std::mem::take(&mut a.rest),
            &data_dir,
            &config_dir,
        ));
    }
    if cmd == "node" && a.rest.first().map(String::as_str) == Some("export-runtime") {
        let mut rest = std::mem::take(&mut a.rest);
        rest.remove(0);
        exit(node::export_runtime(rest, &data_dir, &config_dir));
    }
    if cmd == "remote" && a.rest.first().map(String::as_str) == Some("setup") {
        exit(remote::run(std::mem::take(&mut a.rest), &config_dir));
    }
    if cmd == "remote" {
        // Validate the subcommand before the daemon lookup, so a typo gets the
        // command list even when no daemon is running.
        match a.rest.first().map(String::as_str) {
            None | Some("on" | "off" | "status" | "mode") => {}
            Some(other) => die(&format!(
                "unknown remote command {other:?}: {}\n\n{HELP}",
                remote::COMMANDS
            )),
        }
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
        let project = a.opt("--project");
        let file = PathBuf::from(a.pos().unwrap_or_else(|| ".mason/mason.toml".into()));
        finish(&a);
        // With --project, the step types the project's approved extensions provide are
        // known too, so a plugin step validates offline.
        let loaded = project.as_ref().map(|id| {
            mason_daemon::config::load_project(&config_dir, id).unwrap_or_else(|e| die(&e))
        });
        let plugins = match &loaded {
            Some(p) => mason_daemon::plugins::approved_types(p),
            None => vec![],
        };
        // Whether the project has a full-test schedule with a trigger, for `narrow = true`.
        let full_test = loaded
            .as_ref()
            .map(|p| mason_daemon::schedule::has_full_test_trigger(&p.schedules()));
        exit(if sub == "check" {
            let c = definition::check(&file, &plugins);
            c.max(definition::check_narrow(&file, full_test))
        } else {
            definition::dry(&file, scenarios.as_deref())
        });
    }

    // A word that is no remote command is the typist's error, whether or not masond runs.
    if cmd == "remote" {
        if let Some(other) = a.rest.first().filter(|w| !w.starts_with('-')) {
            if !matches!(other.as_str(), "on" | "off" | "status" | "mode") {
                die(&format!(
                    "unknown remote command {other:?}: {}\n\n{HELP}",
                    remote::COMMANDS
                ));
            }
        }
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
        let origin = a.opt("--origin");
        let outside = outside_args(&mut a);
        finish(&a);
        let command = run_tail.filter(|c| !c.is_empty()).unwrap_or_else(|| {
            die("give the command after `--`: mason run --needs compile -- cargo build")
        });
        exit(
            resources::run(&d, uses, label, &command, outside.as_ref(), origin)
                .unwrap_or_else(|e| die(&e)),
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
    if cmd == "node" {
        node::run(&mut a, &d, json_out);
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
    if cmd == "remote" {
        let sub = a.pos().unwrap_or_else(|| "status".into());
        let mut project: Option<String> = None;
        let to = match sub.as_str() {
            "on" => Some("remote".to_string()),
            "off" => Some("local".to_string()),
            "status" => None,
            "runs" => {
                finish(&a);
                let r = or_die(d.call("GET", "/remote/runs", None));
                if json_out {
                    return println!("{r:#}");
                }
                remote::print_runs(&r);
                return;
            }
            "mode" => {
                let m = a.pos().unwrap_or_else(|| {
                    die("which mode? local, remote, favor-local or favor-remote")
                });
                if !matches!(
                    m.as_str(),
                    "local" | "remote" | "favor-local" | "favor-remote"
                ) {
                    die("which mode? local, remote, favor-local or favor-remote");
                }
                project = a.opt("--project");
                Some(m)
            }
            other => die(&format!(
                "unknown remote command {other:?}: {}\n\n{HELP}",
                remote::COMMANDS
            )),
        };
        finish(&a);
        if let Some(to) = &to {
            // Changing where runs go is the owner's act: say who.
            if std::env::var("MASON_AS").is_err() {
                if let Ok(u) = std::env::var("USER") {
                    std::env::set_var("MASON_AS", u);
                }
            }
            let r = or_die(d.call(
                "POST",
                "/remote",
                Some(&json!({"mode": to, "project": project})),
            ));
            if json_out {
                return println!("{r:#}");
            }
            match &project {
                Some(p) => println!(
                    "{p}: mode {to} (written to its registration file and re-read; the next run uses it)"
                ),
                None => println!(
                    "mode {to} for every project that follows the global mode (the next run, no restart)"
                ),
            }
        }
        let r = or_die(d.call("GET", "/remote", None));
        if json_out && to.is_none() {
            return println!("{r:#}");
        }
        println!(
            "mode {}{}",
            r["mode"].as_str().unwrap_or("?"),
            match (r["changed_by"].as_str(), r["changed_at"].as_i64()) {
                (Some(who), Some(at)) => format!(" (changed by {who} at {at} ms since the epoch)"),
                _ => String::new(),
            }
        );
        if let Some(p) = r["problem"].as_str() {
            println!("  PROBLEM: {p} (treated as local)");
        }
        for p in r["projects"].as_array().into_iter().flatten() {
            println!(
                "  {:<16} {}",
                p["project"].as_str().unwrap_or("?"),
                match p["runs_on"].as_str() {
                    Some(r) => format!(
                        "{} (from {}; GitHub runner {r})",
                        p["mode"].as_str().unwrap_or("?"),
                        p["source"].as_str().unwrap_or("?")
                    ),
                    None if p["nodes_only"].as_bool() == Some(true) => format!(
                        "{} (nodes only: no GitHub setup)",
                        p["mode"].as_str().unwrap_or("?")
                    ),
                    None => "local (no [remote_runs]: always runs here)".to_string(),
                }
            );
        }
        return;
    }
    if cmd == "cpu" {
        finish(&a);
        resources::cpu(&d, json_out).unwrap_or_else(|e| die(&e));
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
            Some("gate") => {
                let name = a
                    .pos()
                    .unwrap_or_else(|| die("which gate? e.g. compile_cpu_max"));
                let reset = a.flag("--reset");
                let value: Option<i64> =
                    if reset {
                        None
                    } else {
                        Some(a.pos().and_then(|u| u.parse().ok()).unwrap_or_else(|| {
                            die("give the new value as a whole number, or --reset")
                        }))
                    };
                finish(&a);
                // Changing a gate is the owner's act: say who.
                if std::env::var("MASON_AS").is_err() {
                    if let Ok(u) = std::env::var("USER") {
                        std::env::set_var("MASON_AS", u);
                    }
                }
                resources::gate(&d, &name, value).unwrap_or_else(|e| die(&e));
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
        "ssh-key" => {
            let sub = a
                .pos()
                .unwrap_or_else(|| die("ssh-key list | ssh-key use <path>"));
            let project = project_of(&mut a);
            // Choosing a key is the owner's act: say who.
            if std::env::var("MASON_AS").is_err() {
                if let Ok(u) = std::env::var("USER") {
                    std::env::set_var("MASON_AS", u);
                }
            }
            match sub.as_str() {
                "list" => {
                    let json = a.flag("--json");
                    finish(&a);
                    let r = or_die(d.call("GET", &format!("/projects/{project}/ssh-keys"), None));
                    if json {
                        println!("{}", serde_json::to_string_pretty(&r).unwrap_or_default());
                    } else {
                        for k in r["keys"].as_array().into_iter().flatten() {
                            let mut tags = vec![];
                            if k["on_owner_account"].as_bool().unwrap_or(false) {
                                tags.push("on the repository owner's account".to_string());
                            }
                            if let Some(u) = k["seen_as"].as_str() {
                                tags.push(format!("seen as {u}"));
                            }
                            if k["encrypted"].as_bool().unwrap_or(false) {
                                tags.push("passphrase".into());
                            }
                            if k["in_agent"].as_bool().unwrap_or(false) {
                                tags.push("in the agent".into());
                            }
                            println!(
                                "{}  {}  {}  [{}]",
                                k["path"].as_str().unwrap_or("(agent only)"),
                                k["fingerprint"].as_str().unwrap_or(""),
                                k["comment"].as_str().unwrap_or(""),
                                tags.join(", ")
                            );
                        }
                    }
                }
                "use" => {
                    let path = a.pos().unwrap_or_else(|| die("which key file?"));
                    finish(&a);
                    let r = or_die(d.call(
                        "POST",
                        &format!("/projects/{project}/ssh-key"),
                        Some(&json!({"path": path})),
                    ));
                    println!(
                        "{project} now pushes with {}. The remote accepted it; landing is released.",
                        r["path"].as_str().unwrap_or(&path)
                    );
                }
                other => die(&format!("unknown ssh-key command {other:?}\n\n{HELP}")),
            }
        }
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
        "project" => {
            let sub = a
                .pos()
                .unwrap_or_else(|| die("project what? reload or rename"));
            if sub == "rename" {
                let yes = a.flag("--yes") || a.flag("-y");
                let old = a
                    .pos()
                    .unwrap_or_else(|| die("rename which project? mason project rename OLD NEW"));
                let new = a
                    .pos()
                    .unwrap_or_else(|| die("rename it to what? mason project rename OLD NEW"));
                finish(&a);
                if std::env::var("MASON_AS").is_err() {
                    std::env::set_var("MASON_AS", owner_identity());
                }
                let r = or_die(d.call(
                    "POST",
                    &format!("/projects/{old}/rename"),
                    Some(&json!({"name": new, "yes": yes})),
                ));
                if json_out {
                    return println!("{r:#}");
                }
                if r["result"] == "refused" {
                    println!("Refused; nothing was changed:");
                    r["errors"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .for_each(|e| println!("  {e}"));
                    exit(1);
                }
                let done = r["result"] == "renamed";
                println!("{}", if done { "Renamed:" } else { "Would:" });
                for x in r["actions"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                {
                    println!("  {x}");
                }
                let kept: Vec<&str> = r["branches_left"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect();
                if !kept.is_empty() {
                    println!("Branches still carrying the old name (left alone):");
                    kept.iter().for_each(|b| println!("  {b}"));
                }
                if done {
                    println!("{old} is now {new}; {} rows rewritten", r["rows"]);
                } else {
                    println!("Nothing was changed. Run `mason project rename {old} {new} --yes` to do it.");
                }
                return;
            }
            if sub != "reload" {
                die(&format!("unknown project command {sub:?}\n\n{HELP}"));
            }
            let project = project_of(&mut a);
            finish(&a);
            let r = or_die(d.call(
                "POST",
                &format!("/projects/{project}/registration/reload"),
                None,
            ));
            if json_out {
                return println!("{r:#}");
            }
            match r["result"].as_str() {
                Some("applied") => println!(
                    "Reloaded {project}.toml; changed: {}. Tasks admitted from now on use it",
                    r["changed"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                Some("refused") => {
                    println!("Refused; the current registration keeps running:");
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
                    if let Some(list) = r["places"].as_array().filter(|l| !l.is_empty()) {
                        println!("  where suites run");
                        for p in list {
                            println!(
                                "      {:<19} {}   <- {}",
                                p["suite"].as_str().unwrap_or("?"),
                                p["text"].as_str().unwrap_or("?"),
                                p["from"].as_str().unwrap_or("?")
                            );
                        }
                    }
                    // Settings of the project file itself (timeouts, idle rules, the stall
                    // watcher): not part of the definition, so not in the steps above.
                    let file = &r["project_file"];
                    if let Some(list) = file["settings"].as_array() {
                        println!("  project file ({})", file["path"].as_str().unwrap_or("?"));
                        for s in list {
                            println!(
                                "      {:<19} {}",
                                s["key"].as_str().unwrap_or("?"),
                                s["value"].as_str().unwrap_or("?")
                            );
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
                        ago_until(p["expires_at"].as_u64().unwrap_or(0), now_ms())
                    );
                }
                if let Some(l) = r["last_lift"].as_object() {
                    println!(
                        "lifted {} by {} via {}",
                        local_clock(l["at"].as_u64().unwrap_or(0)),
                        l["who"].as_str().unwrap_or("?"),
                        l["via"].as_str().unwrap_or("?")
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
                let list = |k: &str| {
                    r[k].as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|v| v.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        })
                        .unwrap_or_default()
                };
                println!("  hosts: {}", list("hosts"));
                println!("  listening on: {}", list("bind"));
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
                        die("mason notify test CHANNEL  (sound, speech, push, email, webhook, apns)")
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
        "session" => {
            // The owner's identity, as `approve` takes it: this user's, unless MASON_AS says otherwise.
            if std::env::var("MASON_AS").is_err() {
                if let Ok(u) = std::env::var("USER") {
                    std::env::set_var("MASON_AS", u);
                }
            }
            let sub = a.pos().unwrap_or_else(|| "list".into());
            let sid = |a: &mut Args| -> u64 {
                let s = a
                    .pos()
                    .unwrap_or_else(|| die("which session? give its number"));
                s.trim_start_matches('#')
                    .parse()
                    .unwrap_or_else(|_| die(&format!("{s:?} is not a session number")))
            };
            let line = |m: &Value| {
                let who = match m["role"].as_str().unwrap_or("?") {
                    "owner" => "you",
                    "agent" => "agent",
                    _ => "  ·",
                };
                println!("{who}: {}", m["text"].as_str().unwrap_or(""));
            };
            match sub.as_str() {
                "list" => {
                    finish(&a);
                    let r = or_die(d.call("GET", "/sessions", None));
                    if json_out {
                        return println!("{r:#}");
                    }
                    let list = r["sessions"].as_array().cloned().unwrap_or_default();
                    if list.is_empty() {
                        println!("no open sessions");
                    }
                    for s in list {
                        println!(
                            "#{:<4} {:<8} task {} · {} turns · {}",
                            s["id"],
                            s["state"].as_str().unwrap_or("?"),
                            s["task"],
                            s["turns"],
                            s["kind"].as_str().unwrap_or("?")
                        );
                    }
                }
                "open" => {
                    let task = task_id(&mut a);
                    let confirm = a.flag("--confirm");
                    finish(&a);
                    let r = or_die(d.call(
                        "POST",
                        &format!("/tasks/{task}/owner-session"),
                        Some(&json!({"confirm": confirm})),
                    ));
                    if json_out {
                        return println!("{r:#}");
                    }
                    println!("session #{}", r["session"]["id"]);
                    if let Some(t) = r["session"]["draft"].as_str() {
                        println!("--- drafted first message ---\n{t}");
                    }
                }
                "show" => {
                    let id = sid(&mut a);
                    let after = a.opt("--after").unwrap_or_else(|| "0".into());
                    finish(&a);
                    let r = or_die(d.call("GET", &format!("/sessions/{id}?after={after}"), None));
                    if json_out {
                        return println!("{r:#}");
                    }
                    let s = &r["session"];
                    println!(
                        "session #{id} ({}) task {} · {} turns",
                        s["state"].as_str().unwrap_or("?"),
                        s["task"],
                        s["turns"]
                    );
                    for m in s["messages"].as_array().cloned().unwrap_or_default() {
                        line(&m);
                    }
                }
                "send" => {
                    let id = sid(&mut a);
                    let wait = !a.flag("--no-wait");
                    let text = a
                        .pos()
                        .unwrap_or_else(|| die("send what? text, or - for stdin"));
                    finish(&a);
                    let text = if text == "-" {
                        let mut t = String::new();
                        let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut t);
                        t
                    } else {
                        text
                    };
                    let before = or_die(d.call("GET", &format!("/sessions/{id}"), None));
                    let seen = before["session"]["messages"]
                        .as_array()
                        .and_then(|m| m.last())
                        .and_then(|m| m["id"].as_u64())
                        .unwrap_or(0);
                    or_die(d.call(
                        "POST",
                        &format!("/sessions/{id}/messages"),
                        Some(&json!({"text": text})),
                    ));
                    if !wait {
                        return println!("sent");
                    }
                    let mut last = seen;
                    loop {
                        std::thread::sleep(std::time::Duration::from_millis(300));
                        let r =
                            or_die(d.call("GET", &format!("/sessions/{id}?after={last}"), None));
                        for m in r["session"]["messages"]
                            .as_array()
                            .cloned()
                            .unwrap_or_default()
                        {
                            last = m["id"].as_u64().unwrap_or(last);
                            if m["role"] != "owner" {
                                line(&m);
                            }
                        }
                        if r["session"]["state"] != "working" {
                            break;
                        }
                    }
                }
                "end" => {
                    let id = sid(&mut a);
                    finish(&a);
                    or_die(d.call("POST", &format!("/sessions/{id}/end"), Some(&json!({}))));
                    println!("ended session #{id}");
                }
                other => die(&format!(
                    "mason session {other:?}: list, open, show, send or end"
                )),
            }
        }
        "health" => {
            let sub = a.pos().unwrap_or_default();
            match sub.as_str() {
                "" => {
                    finish(&a);
                    let r = or_die(d.call("GET", "/health", None));
                    if json_out {
                        return println!("{r:#}");
                    }
                    print_health(&r);
                }
                "pipeline" => {
                    finish(&a);
                    let r = or_die(d.call("GET", "/health/pipeline", None));
                    if json_out {
                        return println!("{r:#}");
                    }
                    let list = r["findings"].as_array().cloned().unwrap_or_default();
                    if list.is_empty() {
                        println!("healthy: no findings");
                    }
                    for f in list {
                        println!(
                            "[{}] {} ({}): {}",
                            f["severity"].as_str().unwrap_or("?"),
                            f["key"].as_str().unwrap_or("?"),
                            f["category"].as_str().unwrap_or("?"),
                            f["summary"].as_str().unwrap_or("")
                        );
                        for e in f["evidence"].as_array().into_iter().flatten() {
                            println!("    {}", e.as_str().unwrap_or(""));
                        }
                    }
                    if let Some(s) = r["session"].as_u64() {
                        println!("health session #{s} is open (mason session show {s})");
                    }
                }
                "check" => {
                    finish(&a);
                    let r = or_die(d.call("POST", "/health/check", Some(&json!({}))));
                    if json_out {
                        return println!("{r:#}");
                    }
                    match r["session"].as_u64() {
                        Some(s) => println!("started health session #{s}"),
                        None if r["fresh"].as_array().is_some_and(|f| !f.is_empty()) => {
                            println!("no session: {}", r["held_back"].as_str().unwrap_or("?"))
                        }
                        None => println!("nothing new to look at"),
                    }
                }
                "history" => {
                    let limit: usize = a.opt("--limit").and_then(|v| v.parse().ok()).unwrap_or(30);
                    finish(&a);
                    let r = or_die(d.call("GET", &format!("/health/history?limit={limit}"), None));
                    if json_out {
                        return println!("{r:#}");
                    }
                    for h in r["history"].as_array().into_iter().flatten() {
                        println!(
                            "{} {:<9} {} {}",
                            h["at_ms"],
                            h["kind"].as_str().unwrap_or("?"),
                            h["key"].as_str().unwrap_or(""),
                            h["detail"].as_str().unwrap_or("")
                        );
                    }
                }
                other => die(&format!(
                    "mason health {other:?}: pipeline, check or history"
                )),
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
        "report" => {
            let days = a.opt("--days").unwrap_or_else(|| "7".into());
            let project = a.opt("--project");
            let context = match a.pos().as_deref() {
                None => false,
                Some("context") => true,
                Some(other) => {
                    eprintln!("mason report: unknown section `{other}` (only `context`)");
                    std::process::exit(2);
                }
            };
            finish(&a);
            let mut path = format!("/report?days={days}");
            if let Some(p) = project {
                path.push_str(&format!("&project={p}"));
            }
            let r = or_die(d.call("GET", &path, None));
            if json_out {
                return println!("{r:#}");
            }
            if context {
                print_context_report(&r["context"]);
            } else {
                print_report(&r);
            }
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
                            "{:<28} {:<12} every {:<10} runs {:<4} {}{}",
                            s["name"].as_str().unwrap_or("?"),
                            s["kind"].as_str().unwrap_or("?"),
                            every_text(s["every_ms"].as_u64().unwrap_or(0)),
                            s["runs"],
                            s["last_status"].as_str().unwrap_or(""),
                            if s["follow"].is_object() {
                                s["follow"]["run"]
                                    .as_u64()
                                    .map_or(" [following]".to_string(), |r| {
                                        format!(" [following run {r}]")
                                    })
                            } else {
                                String::new()
                            }
                        );
                        if let Some(line) = s["full"]["line"].as_str() {
                            println!("    {line}");
                        }
                    }
                }
                "run" => {
                    let force = a.flag("--force");
                    let name = a
                        .pos()
                        .unwrap_or_else(|| die("usage: mason schedule run <name> [--force]"));
                    finish(&a);
                    let r = or_die(d.call(
                        "POST",
                        "/schedules/run",
                        Some(&json!({"name": name, "force": force})),
                    ));
                    println!("{}", r["said"].as_str().unwrap_or(""));
                }
                _ => die("usage: mason schedule list | run <name> [--force]"),
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
                    if let (Some(since), Some(next)) =
                        (r["held_since_ms"].as_u64(), r["next_window_ms"].as_u64())
                    {
                        println!(
                            "held      update waiting since {} (next window {})",
                            local_clock(since),
                            local_clock(next)
                        );
                    }
                    for l in r["plan"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|l| l.as_str())
                    {
                        println!("auto      {l}");
                    }
                    if let Some(l) = r["cost_now"].as_str() {
                        println!("cost      {l}");
                    }
                    for n in r["nodes"].as_array().into_iter().flatten() {
                        println!(
                            "node      {:<16} {}",
                            n["name"].as_str().unwrap_or("?"),
                            n["text"].as_str().unwrap_or("?")
                        );
                    }
                    if let Some(w) = r["waiting"].as_str().filter(|w| !w.is_empty()) {
                        println!("waiting   {w}");
                        println!("          `mason update --now` takes over at once and abandons those batches");
                    }
                }
                Some("ack") => {
                    finish(&a);
                    or_die(d.call("POST", "/update/ack", Some(&json!({}))));
                    println!("acknowledged");
                }
                _ => {
                    // `mason update [--channel C | --sha S] [--rollback] [--check] [--now]`
                    let channel = a.opt("--channel");
                    let sha = a.opt("--sha");
                    let rollback = a.flag("--rollback");
                    let check = a.flag("--check");
                    // Take over at once, abandoning any landing batch still running.
                    let now = a.flag("--now");
                    // Ask as a follower would: the gap between updates applies.
                    let auto = a.flag("--auto");
                    finish(&a);
                    let mut body =
                        json!({"rollback": rollback, "check": check, "now": now, "auto": auto});
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
                    let end = std::time::Instant::now() + std::time::Duration::from_secs(7800);
                    loop {
                        std::thread::sleep(std::time::Duration::from_millis(500));
                        if std::time::Instant::now() > end {
                            die("gave up waiting after two hours");
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
                    if let Some(rule) = p["decided_by"].as_str().filter(|r| !r.is_empty()) {
                        println!("decided by: {rule}");
                    }
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
                let queued = p["queued"].as_array().map_or(0, Vec::len);
                let moving: Vec<String> = p["collecting_for"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|t| t.as_u64().map(|t| format!("#{t}")))
                    .collect();
                if queued > 0 && p["running_batches"].as_u64().unwrap_or(0) == 0 {
                    if moving.is_empty() {
                        println!("  forming now: nothing else is moving");
                    } else {
                        println!(
                            "  collecting for {} s while {} still moving",
                            p["queue_idle_ms"].as_u64().unwrap_or(0) / 1000,
                            moving.join(", ")
                        );
                    }
                }
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
                // Where each of the batch's runs went, with its link when it went to GitHub (#702).
                let prefix = format!("b{}-", b["id"]);
                for run in r["runs"].as_array().into_iter().flatten() {
                    let id = run["id"].as_str().unwrap_or("");
                    if id.starts_with(&prefix) {
                        println!(
                            "    {} ({id}): {}",
                            run["suite"].as_str().unwrap_or("?"),
                            run["where"].as_str().unwrap_or("?")
                        );
                    }
                }
                if b["ci"].is_object() {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, |t| t.as_millis() as u64);
                    println!("    {}", batch_ci::ci_line(&b["ci"], now));
                }
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
                    if x["no_effect"] == json!(true) {
                        println!("      no effect: only a skip of the test step is honoured");
                    }
                    if let Some(how) = x["how"].as_str().filter(|h| !h.is_empty()) {
                        println!("      paid: {how}");
                    }
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
                let ledger = r["ledger"].as_array().cloned().unwrap_or_default();
                if list.is_empty() && ledger.is_empty() {
                    println!("nothing is quarantined");
                }
                for x in ledger {
                    println!(
                        "{} :: {}   known red ({}), fix {}, since {}, last seen {}{}",
                        x["suite"].as_str().unwrap_or("?"),
                        x["test"].as_str().unwrap_or("?"),
                        x["source"].as_str().unwrap_or("?"),
                        x["fix_task"]
                            .as_u64()
                            .map_or("none".to_string(), |n| format!("task #{n}")),
                        ago(x["since"].as_u64().unwrap_or(0)),
                        ago(x["last_seen_ms"].as_u64().unwrap_or(0)),
                        x["note"]
                            .as_str()
                            .filter(|n| !n.is_empty())
                            .map_or(String::new(), |n| format!("   {n}"))
                    );
                }
                for x in list {
                    println!(
                        "{} :: {}   owner {}, expires {}   {}",
                        x["suite"].as_str().unwrap_or("?"),
                        x["test"].as_str().unwrap_or("?"),
                        x["owner"].as_str().unwrap_or("?"),
                        ago_until(x["expires_at"].as_u64().unwrap_or(0), now_ms()),
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
        "hosts" => {
            finish(&a);
            let s = or_die(d.call("GET", "/status", None));
            if json_out {
                return println!("{:#}", s["hosts"]);
            }
            for l in hosts::lines(&s["hosts"]) {
                println!("{l}");
            }
        }
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
            if let Some(l) = s["update_wait"].as_str() {
                println!("update: {l}");
            }
            for p in s["places"].as_array().into_iter().flatten() {
                println!(
                    "where: #{} {} {}: {}",
                    p["task"].as_u64().unwrap_or(0),
                    p["kind"].as_str().unwrap_or("?"),
                    p["name"].as_str().unwrap_or("?"),
                    p["why"].as_str().unwrap_or("?")
                );
            }
            println!(
                "running: {} job(s) of {} allowed, {} step host(s)",
                s["jobs"], s["max_concurrent"], s["step_hosts_running"]
            );
            if let Ok(r) = d.call("GET", "/resources", None) {
                for l in resources::outside_lines(&r, r["now_ms"].as_u64().unwrap_or(0)) {
                    println!("other pipeline: {l}");
                }
            }
            if let Some(l) = agents_line(&s) {
                println!("agents: {l}");
            }
            for a in s["active"].as_array().into_iter().flatten() {
                println!(
                    "active: #{} {} on {}{}",
                    a["task"],
                    a["step"].as_str().unwrap_or("?"),
                    a["display"].as_str().unwrap_or("?"),
                    match a["state"].as_str() {
                        Some("working") => " (working)".to_string(),
                        Some("waiting_check") => format!(
                            " (waiting on a check {})",
                            match a["where"].as_str().unwrap_or("") {
                                "github" => "on GitHub".to_string(),
                                w => match w.strip_prefix("node:") {
                                    Some(n) => format!("on node {n}"),
                                    None => "here".to_string(),
                                },
                            }
                        ),
                        Some("waiting_slot") => format!(
                            " (waiting for {})",
                            a["resource"].as_str().unwrap_or("a slot")
                        ),
                        _ => String::new(),
                    }
                );
            }
            if let Some(l) = github_line(&s["remote"]) {
                println!("on GitHub: {l}");
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
            for (name, w) in s["ci_watch"].as_object().into_iter().flatten() {
                print_ci_watch(name, w);
            }
            for r in s["ci_runs"].as_array().into_iter().flatten() {
                println!(
                    "scheduled CI run in flight: {} {} (waiting for {})",
                    r["name"].as_str().unwrap_or("?"),
                    r["run"]
                        .as_u64()
                        .map_or("(run not listed yet)".to_string(), |n| format!("run {n}")),
                    r["waiting_for"].as_str().unwrap_or("?")
                );
            }
            for f in s["full_tests"].as_array().into_iter().flatten() {
                println!(
                    "{}/{}: {}",
                    f["project"].as_str().unwrap_or("?"),
                    f["schedule"].as_str().unwrap_or("?"),
                    f["line"].as_str().unwrap_or("?")
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
                    let run_on = a.opt("--run-on");
                    if let Some(r) = &run_on {
                        if !matches!(
                            r.as_str(),
                            "github" | "local" | "remote" | "favor-local" | "favor-remote"
                        ) {
                            die("--run-on takes local, remote, favor-local or favor-remote (leave it out to follow the project or global mode)");
                        }
                    }
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
                    let r = or_die(d.call("POST", "/tasks", Some(&json!({"project": project, "title": title, "body": body, "priority": priority, "proposed": proposed, "explored": explored, "after": after, "run_on": run_on}))));
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
                    let original = a.flag("--original");
                    finish(&a);
                    let t = or_die(d.call("GET", &format!("/tasks/{id}"), None));
                    if original {
                        match t["edited"].as_object() {
                            Some(e) => {
                                println!("#{id} {}", e["original_title"].as_str().unwrap_or(""));
                                println!("{}", e["original_body"].as_str().unwrap_or(""));
                            }
                            None => println!("Task #{id} has not been edited."),
                        }
                        return;
                    }
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
                    if let Some(e) = t["edited"].as_object() {
                        let why = e["reason"].as_str().filter(|r| !r.is_empty());
                        println!(
                            "  edited    {} time(s); last by {} at {} ms{} (the text as filed: mason task show {} --original)",
                            e["count"],
                            e["who"].as_str().unwrap_or("?"),
                            e["at"],
                            why.map(|r| format!(": {r}")).unwrap_or_default(),
                            t["id"]
                        );
                    }
                    println!("  activity  {}", t["activity"].as_str().unwrap_or("?"));
                    if let Some(m) = t["model"]["display"].as_str() {
                        println!("  model     {m}");
                        if let Some(w) = t["model"]["why"].as_str().filter(|w| !w.is_empty()) {
                            println!("            ({w})");
                        }
                    }
                    for l in run_lines(&t) {
                        println!("{l}");
                    }
                    if let Some(l) = t["run_on"]["line"].as_str() {
                        println!("  {l}");
                    }
                    if let Some(l) = hosts::running_line(&t["running_on"]) {
                        println!("  {l}");
                    }
                    for p in t["places"].as_array().into_iter().flatten() {
                        println!(
                            "  where     {} {}: {}",
                            p["kind"].as_str().unwrap_or("?"),
                            p["name"].as_str().unwrap_or("?"),
                            p["why"].as_str().unwrap_or("?")
                        );
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
                        if let Some(r) = t["diagnosis_ran_on"].as_str() {
                            println!("  diagnosis: {r}");
                        }
                        if t["diagnosis_running"].is_object() {
                            println!(
                                "  diagnosis running since {} ms, on {}",
                                t["diagnosis_running"]["started_ms"],
                                t["diagnosis_running"]["ran_on"].as_str().unwrap_or("?")
                            );
                        }
                        if let Some(n) = t["auto_retries"].as_u64() {
                            let next = match t["next_auto_retry_ms"].as_u64() {
                                Some(at) => {
                                    let now = t["progress"]["now_ms"].as_u64().unwrap_or(at);
                                    format!(
                                        "the next one in {}",
                                        mason_core::progress_text::duration(at.saturating_sub(now))
                                    )
                                }
                                None => "no more will be made".to_string(),
                            };
                            println!(
                                "  auto      {n} automatic retr{} in the last 24 h; {next}",
                                if n == 1 { "y" } else { "ies" }
                            );
                        }
                    }
                    for l in t["excused"].as_array().into_iter().flatten() {
                        println!("  excused   {}", l.as_str().unwrap_or(""));
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
                    for l in t["review_findings"].as_array().into_iter().flatten() {
                        println!("  review    {}", l.as_str().unwrap_or(""));
                    }
                    for n in t["owner_notes"].as_array().into_iter().flatten() {
                        println!(
                            "  owner note ({}) {}",
                            n["step"].as_str().unwrap_or("?"),
                            n["note"].as_str().unwrap_or("")
                        );
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
                            "  run {:<3} {:<11} {:<12} attempt {}  {}{}",
                            s["run"],
                            s["step"].as_str().unwrap_or("?"),
                            s["name"].as_str().unwrap_or(""),
                            s["attempt"],
                            s["outcome"].as_str().unwrap_or("running"),
                            s["context"]["line"]
                                .as_str()
                                .map(|l| format!("  {l}"))
                                .unwrap_or_default()
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
                    let restart = a.flag("--restart");
                    let message = a.opt("--message");
                    finish(&a);
                    // Without --from the daemon picks: the failed step when the
                    // failure was not about the code, otherwise developing. Say
                    // which before sending.
                    // The run shown is sent back, so a second retry of the same
                    // failure is refused.
                    let t = or_die(d.call("GET", &format!("/tasks/{id}"), None));
                    if from.is_none() {
                        if let Some(to) = t["retry_to"].as_str() {
                            println!("Task #{id} will retry from `{to}`");
                        }
                    }
                    if t["step"].as_str().is_some_and(|s| s.contains("land")) {
                        println!(
                            "Task #{id} is in landing: if its batch is running it leaves the batch, and its new commits will not join it"
                        );
                    }
                    let mut body = from.as_ref().map_or(json!({}), |f| json!({"to": f}));
                    if let Some(run) = t["last_run"].as_u64() {
                        body["seen"] = json!(run);
                    }
                    if restart {
                        body["restart"] = json!(true);
                    }
                    if let Some(m) = message {
                        body["message"] = json!(m);
                    }
                    let r = or_die(d.call("POST", &format!("/tasks/{id}/retry"), Some(&body)));
                    println!(
                        "Retrying task #{id} from {}",
                        r["to"].as_str().unwrap_or("developing")
                    );
                }
                "diagnose" => {
                    let id = task_id(&mut a);
                    finish(&a);
                    or_die(d.call("POST", &format!("/tasks/{id}/diagnose"), Some(&json!({}))));
                    println!("Diagnosis of task #{id} started; see `mason task diagnosis {id}`");
                }
                "diagnosis" => {
                    let id = task_id(&mut a);
                    let what = a.pos();
                    let n = a.pos();
                    finish(&a);
                    match (what.as_deref(), n.and_then(|n| n.parse::<u64>().ok())) {
                        (None, _) => {
                            let r = or_die(d.call("GET", &format!("/tasks/{id}/diagnoses"), None));
                            let rows = r.as_array().cloned().unwrap_or_default();
                            if rows.is_empty() {
                                println!("Task #{id} has no diagnosis.");
                            }
                            for row in rows {
                                println!(
                                    "#{} [{}] {}",
                                    row["id"],
                                    row["status"].as_str().unwrap_or("?"),
                                    row["text"].as_str().unwrap_or("")
                                );
                                if let Some(r) = row["ran_on"].as_str() {
                                    println!("    diagnosis: {r}");
                                }
                            }
                        }
                        (Some(w @ ("apply" | "dismiss")), Some(n)) => {
                            let r = or_die(d.call(
                                "POST",
                                &format!("/tasks/{id}/diagnoses/{n}/{w}"),
                                Some(&json!({})),
                            ));
                            println!("Diagnosis #{n}: {}", r["said"].as_str().unwrap_or("done"));
                        }
                        _ => die("usage: mason task diagnosis <id> [apply|dismiss N]"),
                    }
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
                    let all = a.flag("--all");
                    finish(&a);
                    let mut body = json!({"reason": reason, "owner": owner, "deferred": deferred});
                    match (suite, step, all) {
                        (Some(s), None, false) => body["suite"] = json!(s),
                        (None, Some(s), false) => body["step"] = json!(s),
                        (None, None, true) => body["all"] = json!(true),
                        _ => die("name one suite, one step or all: --suite S, --step S or --all"),
                    }
                    let r = or_die(d.call("POST", &format!("/tasks/{id}/skip"), Some(&body)));
                    if all {
                        println!(
                            "Task #{id} is on the fast track: the suites do not run for it through \
                             landing, but lint, the build floor and the tests its own change adds \
                             still do. Debt #{} owes the full run.",
                            r["debt"]
                        );
                    } else {
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
                }
                "answer" => {
                    let id = task_id(&mut a);
                    let who = a
                        .opt("--as")
                        .or_else(|| std::env::var("MASON_AS").ok())
                        .or_else(|| std::env::var("USER").ok())
                        .unwrap_or_default();
                    let card = a.opt("--card");
                    let context = a.opt("--context");
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
                    let mut body = json!({"answer": answer_text(&answer), "hash": hash, "as": who});
                    if let Some(c) = context {
                        body["context"] = json!(c);
                    }
                    let r = or_die(d.call("POST", &format!("/tasks/{id}/answer"), Some(&body)));
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
                    exit(run_session(&s));
                }
                "defer" | "undefer" => {
                    let id = task_id(&mut a);
                    finish(&a);
                    or_die(d.call("POST", &format!("/tasks/{id}/{sub}"), None));
                    println!(
                        "Task #{id} {}",
                        if sub == "defer" {
                            "deferred"
                        } else {
                            "back in Proposed"
                        }
                    );
                }
                "approve" => {
                    let id = task_id(&mut a);
                    let priority = a.opt("--priority");
                    finish(&a);
                    // A change to MASON's own rules is approved by the owner's identity: this
                    // user's, unless `--as` said otherwise.
                    if std::env::var("MASON_AS").is_err() {
                        if let Ok(u) = std::env::var("USER") {
                            std::env::set_var("MASON_AS", u);
                        }
                    }
                    let body = priority.as_ref().map(|p| json!({"priority": p}));
                    or_die(d.call("POST", &format!("/tasks/{id}/approve"), body.as_ref()));
                    match priority {
                        Some(p) => println!("Approved task #{id} at priority {p}"),
                        None => println!("Approved task #{id}"),
                    }
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
                "notes" => {
                    let id = task_id(&mut a);
                    let clear = a.flag("--clear");
                    finish(&a);
                    if clear {
                        // Owner only, as `priority` is.
                        if std::env::var("MASON_AS").is_err() {
                            if let Ok(u) = std::env::var("USER") {
                                std::env::set_var("MASON_AS", u);
                            }
                        }
                        let r = or_die(d.call(
                            "POST",
                            &format!("/tasks/{id}/owner-notes/clear"),
                            Some(&json!({})),
                        ));
                        println!("Task #{id}: cleared {} owner note(s)", r["cleared"]);
                    } else {
                        let r = or_die(d.call("GET", &format!("/tasks/{id}/owner-notes"), None));
                        let notes = r["notes"].as_array().cloned().unwrap_or_default();
                        if notes.is_empty() {
                            println!("Task #{id} has no owner notes.");
                        }
                        for n in notes {
                            println!(
                                "[{}] {}: {}",
                                n["at_ms"],
                                n["step"].as_str().unwrap_or("?"),
                                n["note"].as_str().unwrap_or("")
                            );
                        }
                    }
                }
                "edit" => {
                    let id = task_id(&mut a);
                    let title = a.opt("--title");
                    let body = if let Some(b) = a.opt("--body") {
                        Some(if b == "-" {
                            let mut s = String::new();
                            let _ = std::io::stdin().read_to_string(&mut s);
                            s
                        } else {
                            b
                        })
                    } else {
                        a.opt("--body-file").map(|f| {
                            std::fs::read_to_string(&f)
                                .unwrap_or_else(|e| die(&format!("cannot read {f}: {e}")))
                        })
                    };
                    let reason = a.opt("--reason").unwrap_or_default();
                    finish(&a);
                    if std::env::var("MASON_AS").is_err() {
                        if let Ok(u) = std::env::var("USER") {
                            std::env::set_var("MASON_AS", u);
                        }
                    }
                    let r = or_die(d.call(
                        "POST",
                        &format!("/tasks/{id}/edit"),
                        Some(&json!({"title": title, "body": body, "reason": reason})),
                    ));
                    println!(
                        "Task #{id} edited ({}). A step already running keeps the prompt it started with; the next step reads the new text.",
                        r["state"].as_str().unwrap_or("?")
                    );
                }
                "priority" => {
                    let id = task_id(&mut a);
                    let level = a
                        .pos()
                        .unwrap_or_else(|| die("which priority? urgent, high, normal or low"));
                    finish(&a);
                    // Owner only: this user's identity, as `unblock` takes it, unless MASON_AS says otherwise.
                    if std::env::var("MASON_AS").is_err() {
                        if let Ok(u) = std::env::var("USER") {
                            std::env::set_var("MASON_AS", u);
                        }
                    }
                    let r = or_die(d.call(
                        "POST",
                        &format!("/tasks/{id}/priority"),
                        Some(&json!({"priority": level})),
                    ));
                    println!(
                        "Task #{id}: priority {level} ({} waiting request(s) re-ranked)",
                        r["reranked"]
                    );
                }
                "update-priority" => {
                    let id = task_id(&mut a);
                    let level = a
                        .pos()
                        .unwrap_or_else(|| die("which update priority? urgent or normal"));
                    finish(&a);
                    if !matches!(level.as_str(), "urgent" | "normal") {
                        die("which update priority? urgent or normal");
                    }
                    // Owner only, as `priority` is.
                    if std::env::var("MASON_AS").is_err() {
                        if let Ok(u) = std::env::var("USER") {
                            std::env::set_var("MASON_AS", u);
                        }
                    }
                    or_die(d.call(
                        "POST",
                        &format!("/tasks/{id}/update-priority"),
                        Some(&json!({"priority": level})),
                    ));
                    println!(
                        "Task #{id}: update priority {level} (the daemon's own update {} when it lands)",
                        if level == "urgent" { "goes at once" } else { "waits for a cheap moment" }
                    );
                }
                "run-on" => {
                    let id = task_id(&mut a);
                    let to = a.pos().unwrap_or_else(|| {
                        die("where? local, remote, favor-local, favor-remote or default")
                    });
                    finish(&a);
                    if !matches!(
                        to.as_str(),
                        "github" | "local" | "remote" | "favor-local" | "favor-remote" | "default"
                    ) {
                        die("where? local, remote, favor-local, favor-remote or default");
                    }
                    // Owner only, as `priority` is.
                    if std::env::var("MASON_AS").is_err() {
                        if let Ok(u) = std::env::var("USER") {
                            std::env::set_var("MASON_AS", u);
                        }
                    }
                    let r = or_die(d.call(
                        "POST",
                        &format!("/tasks/{id}/run_on"),
                        Some(&json!({"to": to})),
                    ));
                    println!(
                        "Task #{id}: {} (from the next suite or check; a run in progress is not moved)",
                        r["run_on"]["line"].as_str().unwrap_or("?")
                    );
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
    println!("Flaky tests (last 7 days)");
    let flaky = r["flaky"].as_object().cloned().unwrap_or_default();
    if flaky.is_empty() {
        println!("  none");
    }
    for (name, n) in flaky {
        println!("  {name} ({n} time(s))");
    }
    if let Some(p) = r["pipeline"].as_object() {
        let n = p["findings"].as_array().map_or(0, Vec::len);
        println!(
            "Pipeline checks: {}",
            if n == 0 {
                "healthy".to_string()
            } else {
                format!("{n} finding(s); `mason health pipeline`")
            }
        );
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
    println!(
        "Stall watch: {} SIGCONT(s) sent to stopped processes, {} silent tree(s) killed",
        r["stall_watch"]["sigconts"], r["stall_watch"]["kills"]
    );
    println!(
        "Monitor sessions closed for not reading: {}",
        r["ws_sessions_closed_for_not_reading"]
            .as_u64()
            .unwrap_or(0)
    );
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

/// `mason report context`: the daemon's `context` section, one line per step and bucket.
fn print_context_report(r: &serde_json::Value) {
    let rows = r.as_array().cloned().unwrap_or_default();
    if rows.is_empty() {
        println!("no agent run in the window reported its context");
        return;
    }
    println!("send-backs by peak context fill:");
    println!("  step         peak fill   runs  sent back   rate");
    for x in &rows {
        println!(
            "  {:<12} {:<9} {:>6} {:>10}  {:>4}%",
            x["step"].as_str().unwrap_or("?"),
            x["bucket"].as_str().unwrap_or("?"),
            x["runs"],
            x["sent_back"],
            x["rate_percent"]
        );
    }
}

/// `mason report` as text: one block per section, from the daemon's `/report`.
fn print_report(r: &serde_json::Value) {
    let ms = |v: &serde_json::Value| {
        v.as_u64()
            .map_or("-".to_string(), |m| format!("{}s", m / 1000))
    };
    println!(
        "landed {}  failed {}",
        r["landed"]["total"], r["failed"]["total"]
    );
    for (title, key) in [("landed", "landed"), ("failed", "failed")] {
        if let Some(days) = r[key]["per_day"].as_object() {
            for (day, ps) in days {
                println!("  {title} {day}: {ps}");
            }
        }
    }
    let l = &r["lead_time_ms"];
    println!(
        "lead time (admitted to landed): median {}  p90 {}  ({} tasks)",
        ms(&l["median"]),
        ms(&l["p90"]),
        l["samples"]
    );
    println!("send-backs: {}", r["send_backs"]["total"]);
    for (k, v) in r["send_backs"]["by_reason"]
        .as_object()
        .into_iter()
        .flatten()
    {
        println!("  {v:>4}  {k}");
    }
    for (k, v) in r["send_backs"]["by_suite"]
        .as_object()
        .into_iter()
        .flatten()
    {
        println!("  suite {k}: {v}");
    }
    println!("task failures:");
    for f in r["failures"].as_array().into_iter().flatten() {
        println!(
            "  {:>4}  [{}] {}",
            f["count"],
            f["class"].as_str().unwrap_or(""),
            f["reason"].as_str().unwrap_or("")
        );
    }
    println!("waits:");
    for w in r["waits"].as_array().into_iter().flatten() {
        println!(
            "  {:<16} count {:>4}  total {:>8}  median {}",
            w["class"].as_str().unwrap_or(""),
            w["count"],
            ms(&w["total_ms"]),
            ms(&w["median_ms"])
        );
    }
    println!("owner interventions: {}", r["interventions"]["total"]);
    for (k, v) in r["interventions"]["by_kind"]
        .as_object()
        .into_iter()
        .flatten()
    {
        println!("  {v:>4}  {k}");
    }
    let b = &r["batches"];
    if b["total"].as_u64().is_some_and(|n| n > 0) {
        println!(
            "landing batches: {}  landed {}  red {}  timed out {} (twice {})",
            b["total"], b["landed"], b["red"], b["timed_out"], b["timed_out_twice"]
        );
    }
    println!(
        "host losses {}  flaky sightings {}  open debts {}",
        r["host_losses"],
        r["flaky"]["sightings"],
        r["open_debts"].as_array().map_or(0, Vec::len)
    );
    let slow = r["slowest_tests"].as_array().cloned().unwrap_or_default();
    if !slow.is_empty() {
        println!("slowest tests (median run time of the last runs, without the wait for a test world; share of the suite's tests):");
        for t in &slow {
            println!(
                "  {:>8}  {:>5}%  {}::{}",
                format!("{:.2}s", t["median_ms"].as_f64().unwrap_or(0.0) / 1000.0),
                t["share_percent"],
                t["suite"].as_str().unwrap_or(""),
                t["test"].as_str().unwrap_or("")
            );
        }
    }
}

/// The per-project numbers of the digest (s255): p90 beside the median, send-backs per landed
/// task by class, retries by failure class, test-step run times, and what is not recorded.
fn print_digest_projects(r: &serde_json::Value) {
    let Some(m) = r["per_project"].as_object().filter(|m| !m.is_empty()) else {
        return;
    };
    println!("By project:");
    for (name, p) in m {
        println!("  {name}: {} landed", p["landed"]);
        let c = &p["claim_to_land"];
        match (c["median_ms"].as_u64(), c["p90_ms"].as_u64()) {
            (Some(med), Some(p90)) => println!(
                "    claim to land: median {}, p90 {} ({} task(s))",
                ms_text(med),
                ms_text(p90),
                c["samples"]
            ),
            _ => println!("    claim to land: no landed task in the window"),
        }
        let by = p["send_backs"]["by_class"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        let per = p["send_backs"]["per_landed"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        let parts: Vec<String> = by
            .iter()
            .map(|(k, n)| {
                let rate = per[k]
                    .as_f64()
                    .map_or("n/a".to_string(), |v| format!("{v:.2}"));
                format!("{k} {n} ({rate} per landed)")
            })
            .collect();
        println!("    send-backs: {}", parts.join(", "));
        let retries = p["retries_by_failure_class"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        if retries.is_empty() {
            println!("    retries: none");
        } else {
            let parts: Vec<String> = retries
                .iter()
                .map(|(k, v)| format!("{k}: {} by hand, {} automatic", v["manual"], v["auto"]))
                .collect();
            println!("    retries: {}", parts.join("; "));
        }
        let t = &p["test_step_run_ms"];
        match (t["median"].as_u64(), t["p90"].as_u64()) {
            (Some(med), Some(p90)) => println!(
                "    test step run: median {}, p90 {} ({} run(s))",
                ms_text(med),
                ms_text(p90),
                t["runs"]
            ),
            _ => println!("    test step run: none in the window"),
        }
        println!("    run time per suite: not recorded");
        println!("    run_check calls per task: not recorded");
    }
}

/// The step runs `task show` lists: each launch, and under its run what the agent said it
/// tried and gave up. The agent's words are printed on one line, without control characters.
fn run_lines(t: &serde_json::Value) -> Vec<String> {
    let plain = |v: &serde_json::Value| {
        v.as_str()
            .unwrap_or("")
            .replace(|c: char| c.is_control(), " ")
    };
    let mut out = vec![];
    for r in t["steps"].as_array().into_iter().flatten() {
        let launches = r["launches"].as_array().map(Vec::as_slice).unwrap_or(&[]);
        for l in launches {
            out.push(format!(
                "  run {:<4} {:<9} {}",
                r["run"],
                l["kind"].as_str().unwrap_or(""),
                l["display"].as_str().unwrap_or("")
            ));
        }
        let skills: Vec<&str> = r["skills"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|s| s.as_str())
            .collect();
        if !skills.is_empty() {
            out.push(format!(
                "  run {:<4} skills    {}",
                r["run"],
                skills.join(", ")
            ));
        }
        let tried: Vec<&serde_json::Value> = t["tried"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|x| x["run"] == r["run"])
            .flat_map(|x| x["tried"].as_array().into_iter().flatten())
            .collect();
        if launches.is_empty() && !tried.is_empty() {
            out.push(format!("  run {:<4} {}", r["run"], plain(&r["name"])));
        }
        for e in tried {
            out.push(format!(
                "    tried   {} (dropped: {})",
                plain(&e["approach"]),
                plain(&e["why_dropped"])
            ));
        }
    }
    out
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
    print_digest_projects(r);
    println!("Time by state:");
    if let Some(m) = r["time_by_state_ms"].as_object() {
        for (k, v) in m {
            println!("  {k:<20} {}", ms_text(v.as_u64().unwrap_or(0)));
        }
    }
    println!("Waiting, by what was waited for:");
    for row in r["wait_by_class"].as_array().cloned().unwrap_or_default() {
        let count = row["count"]
            .as_u64()
            .map_or("-".to_string(), |c| c.to_string());
        let longest = match (row["longest_ms"].as_u64(), row["longest_task"].as_u64()) {
            (Some(ms), Some(t)) => format!("longest {} (#{t})", ms_text(ms)),
            _ => String::new(),
        };
        let share = row["share"]
            .as_f64()
            .map_or(String::new(), |s| format!("{:.0}% of task time", s * 100.0));
        println!(
            "  {:<12} {:<22} {:>4} wait(s)  total {:<7} {longest}  {share}",
            row["project"].as_str().unwrap_or("?"),
            row["class"].as_str().unwrap_or("?"),
            count,
            ms_text(row["total_ms"].as_u64().unwrap_or(0)),
        );
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

/// The base-branch CI watch of one project, as `mason status` prints it.
fn print_ci_watch(project: &str, w: &Value) {
    let list = |v: &Value| {
        v.as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(", ")
    };
    let watched = list(&w["workflows"]);
    println!(
        "ci watch for {project}: {}{}{}",
        if watched.is_empty() {
            "no workflow seen yet".to_string()
        } else {
            format!("watching {watched}")
        },
        w["gate"]
            .as_str()
            .map_or(String::new(), |g| format!("; gate {g}")),
        if w["excluded"].as_array().is_some_and(|e| !e.is_empty()) {
            format!("; excluding {}", list(&w["excluded"]))
        } else {
            String::new()
        }
    );
    if let Some(e) = w["error"].as_str() {
        println!("  CANNOT LOOK: {e}");
    }
    if let Some(f) = w["freeze"].as_object() {
        println!(
            "  LANDING FROZEN: {} (fix task #{})",
            f["reason"].as_str().unwrap_or("?"),
            f["fix_task"]
        );
    }
    for p in w["pending"].as_array().into_iter().flatten() {
        println!(
            "  red, waiting for agreement: {} ({} of {} runs)",
            p["job"].as_str().unwrap_or("?"),
            p["red_runs"],
            p["agreement"]
        );
    }
    for h in w["held_back"].as_array().into_iter().flatten() {
        println!("  {}", h["text"].as_str().unwrap_or("?"));
    }
    if let Some(t) = w["unconfirmed_text"].as_str() {
        println!("  {t}");
    }
    for t in w["timeouts"].as_array().into_iter().flatten() {
        println!(
            "  hit its time limit: {} ({} of {} runs{})",
            t["job"].as_str().unwrap_or("?"),
            t["timed_out_runs"],
            t["agreement"],
            if t["filed"] == true { ", filed" } else { "" }
        );
    }
    for f in w["filed"].as_array().into_iter().flatten().take(5) {
        println!(
            "  filed: {} as {}{}{}",
            f["job"].as_str().unwrap_or("?"),
            f["task"]
                .as_u64()
                .map_or("no task".to_string(), |t| format!("#{t}")),
            if f["urgent"] == true { " (urgent)" } else { "" },
            if f["proposed"] == true {
                " (proposed)"
            } else if f["auto"] == true {
                " (auto-approved: cancel it if it is wrong)"
            } else {
                ""
            },
        );
    }
}

// The fake provider is a unix shell script, so these tests only build on unix.
#[cfg(all(test, unix))]
mod open_session_tests {
    use super::*;
    use serde_json::json;
    use std::os::unix::fs::PermissionsExt;

    /// A fake provider binary that records its cwd and arguments.
    fn fake(dir: &std::path::Path, name: &str) -> String {
        let p = dir.join(name);
        std::fs::write(
            &p,
            format!(
                "#!/bin/sh\npwd -P > '{d}/{name}.cwd'\necho \"$@\" > '{d}/{name}.args'\n",
                d = dir.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p.display().to_string()
    }

    /// Run the session, retrying only when the fake never ran (it writes
    /// `<provider>.cwd` first) and the spawn failed with 127: a spawn racing a
    /// sibling test's fork can fail with ETXTBSY.
    fn run_retrying(s: &serde_json::Value, dir: &std::path::Path, provider: &str) -> i32 {
        let mut code = run_session(s);
        for _ in 0..50 {
            if code != 127 || dir.join(format!("{provider}.cwd")).exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
            code = run_session(s);
        }
        code
    }

    fn check(provider: &str, resume: &str) {
        let d = tempfile::tempdir().unwrap();
        let wt = d.path().join("wt");
        std::fs::create_dir(&wt).unwrap();
        let bin = fake(d.path(), provider);
        let s = json!({"worktree": wt, "provider": provider, "session": "sid-1",
            "argv": [bin, resume, "sid-1"]});
        assert_eq!(run_retrying(&s, d.path(), provider), 0);
        let cwd = std::fs::read_to_string(d.path().join(format!("{provider}.cwd"))).unwrap();
        assert_eq!(cwd.trim(), wt.canonicalize().unwrap().display().to_string());
        let args = std::fs::read_to_string(d.path().join(format!("{provider}.args"))).unwrap();
        assert_eq!(args.trim(), format!("{resume} sid-1"));
    }

    #[test]
    fn open_resumes_a_claude_session_in_the_worktree() {
        check("claude", "--resume");
    }

    #[test]
    fn open_resumes_a_codex_session_in_the_worktree() {
        check("codex", "resume");
    }

    #[test]
    fn open_without_a_session_starts_a_fresh_one() {
        let d = tempfile::tempdir().unwrap();
        let bin = fake(d.path(), "codex");
        let s = json!({"worktree": d.path(), "provider": "codex", "session": null, "argv": [bin]});
        assert_eq!(run_retrying(&s, d.path(), "codex"), 0);
        let args = std::fs::read_to_string(d.path().join("codex.args")).unwrap();
        assert_eq!(args.trim(), "");
    }
}

/// "7 agents: 2 working, 5 waiting on GitHub checks" from `mason status --json`; `None` when no
/// agent has a state.
fn agents_line(s: &Value) -> Option<String> {
    let states: Vec<(&str, &str)> = s["active"]
        .as_array()?
        .iter()
        .filter_map(|a| Some((a["state"].as_str()?, a["where"].as_str().unwrap_or(""))))
        .collect();
    if states.is_empty() {
        return None;
    }
    let n = |f: &dyn Fn(&(&str, &str)) -> bool| states.iter().filter(|x| f(x)).count();
    let mut parts = vec![];
    for (c, w) in [
        (n(&|x| x.0 == "working"), "working".to_string()),
        (
            n(&|x| x.0 == "waiting_check" && x.1 == "github"),
            "waiting on GitHub checks".to_string(),
        ),
        (
            n(&|x| x.0 == "waiting_check" && x.1.starts_with("node:")),
            "waiting on checks on nodes".to_string(),
        ),
        (
            n(&|x| x.0 == "waiting_check" && x.1 != "github" && !x.1.starts_with("node:")),
            "waiting on checks here".to_string(),
        ),
        (
            n(&|x| x.0 == "waiting_slot"),
            "waiting for a slot".to_string(),
        ),
    ] {
        if c > 0 {
            parts.push(format!("{c} {w}"));
        }
    }
    let total = states.len();
    Some(format!(
        "{total} agent{}: {}",
        if total == 1 { "" } else { "s" },
        parts.join(", ")
    ))
}

/// "13 of 16 runners: 7 checks, 4 test steps, 2 debts" from the `remote` of `mason status
/// --json`; `None` when nothing is out.
fn github_line(r: &Value) -> Option<String> {
    if r.is_null() {
        return None;
    }
    let pool = |name: &str| {
        r["pools"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|p| p["name"] == name)
            .map(|p| {
                (
                    p["used"].as_u64().unwrap_or(0),
                    p["limit"].as_u64().unwrap_or(0),
                )
            })
    };
    let mut parts = vec![];
    for (k, one, many) in [
        ("checks", "check", "checks"),
        ("test_steps", "test step", "test steps"),
        ("batches", "batch", "batches"),
        ("debts", "debt", "debts"),
    ] {
        let n = r[k].as_u64().unwrap_or(0);
        if n > 0 {
            parts.push(format!("{n} {}", if n == 1 { one } else { many }));
        }
    }
    let head = match pool("gha") {
        Some((u, l)) => format!("{u} of {l} runners"),
        None => "runners".to_string(),
    };
    let mac = pool("gha-macos")
        .filter(|(u, _)| *u > 0)
        .map(|(u, l)| format!("; macOS {u} of {l}"))
        .unwrap_or_default();
    Some(format!("{head}: {}{mac}", parts.join(", ")))
}

/// The owner's identity as the daemon's default owner list takes it: MASON_AS, else USER,
/// else LOGNAME, else "owner".
fn owner_identity() -> String {
    ["MASON_AS", "USER", "LOGNAME"]
        .iter()
        .find_map(|k| std::env::var(k).ok())
        .unwrap_or_else(|| "owner".into())
}

#[cfg(test)]
mod answer_tests {
    use super::answer_text;

    #[test]
    fn a_list_of_options_is_one_answer() {
        assert_eq!(answer_text(&["1,3".to_string()]), "1,3");
        assert_eq!(answer_text(&["1".to_string(), "3".to_string()]), "1 3");
        let mut a = super::Args {
            rest: vec!["12".into(), "1,3".into(), "--as".into(), "me".into()],
        };
        assert_eq!(super::task_id(&mut a), 12);
        assert_eq!(a.opt("--as").as_deref(), Some("me"));
        assert_eq!(answer_text(&a.rest), "1,3");
    }

    #[test]
    fn task_show_lists_what_a_run_tried_under_it() {
        let t = serde_json::json!({
            "steps": [
                {"run": 3, "name": "develop", "launches": [{"kind": "launch", "display": "m"}]},
                {"run": 4, "name": "test", "launches": []},
                {"run": 5, "name": "develop", "launches": []},
            ],
            "tried": [
                {"run": 5, "step": "develop", "tried": [{"approach": "a cache", "why_dropped": "stale\u{1b}[2J"}]},
                {"run": 3, "step": "develop", "tried": [{"approach": "a lock", "why_dropped": "deadlock"}]},
            ],
        });
        let lines = super::run_lines(&t);
        assert_eq!(lines.len(), 4, "{lines:?}");
        assert!(lines[0].starts_with("  run 3"), "{lines:?}");
        assert_eq!(lines[1], "    tried   a lock (dropped: deadlock)");
        assert!(lines[2].starts_with("  run 5") && lines[2].ends_with("develop"));
        assert_eq!(lines[3], "    tried   a cache (dropped: stale [2J)");
    }

    #[test]
    fn agents_and_github_lines() {
        let a = |state: &str, place: &str| serde_json::json!({"state": state, "where": place});
        let mut active = vec![a("working", ""), a("working", "")];
        active.extend((0..5).map(|_| a("waiting_check", "github")));
        let s = serde_json::json!({"active": active});
        assert_eq!(
            super::agents_line(&s).as_deref(),
            Some("7 agents: 2 working, 5 waiting on GitHub checks")
        );
        assert_eq!(super::agents_line(&serde_json::json!({"active": []})), None);
        let r = serde_json::json!({
            "pools": [{"name": "gha", "used": 13, "limit": 16}, {"name": "gha-macos", "used": 2, "limit": 4}],
            "checks": 7, "test_steps": 4, "batches": 0, "debts": 2,
        });
        assert_eq!(
            super::github_line(&r).as_deref(),
            Some("13 of 16 runners: 7 checks, 4 test steps, 2 debts; macOS 2 of 4")
        );
        assert_eq!(super::github_line(&serde_json::Value::Null), None);
    }
}

#[cfg(test)]
mod owner_identity_tests {
    #[test]
    fn owner_identity_follows_the_daemon_default_order() {
        for k in ["MASON_AS", "USER", "LOGNAME"] {
            std::env::remove_var(k);
        }
        assert_eq!(super::owner_identity(), "owner");
        std::env::set_var("USER", "u");
        assert_eq!(super::owner_identity(), "u");
    }
}

#[cfg(test)]
mod ago_until_tests {
    use super::ago_until;

    #[test]
    fn an_expiry_is_rounded_up_never_cut_to_the_hour_below() {
        let now = 1_700_000_000_000;
        let after = |s: u64| ago_until(now + s * 1000, now);
        assert_eq!(after(2 * 3600 - 1), "in 2h");
        assert_eq!(ago_until(now + 2 * 3_600_000 - 1, now), "in 2h");
        assert_eq!(after(59 * 60 + 30), "in 1h");
        assert_eq!(after(2 * 3600 + 59 * 60), "in 2h 59m");
        assert_eq!(after(3600 + 1), "in 1h 1m");
        assert_eq!(after(59 * 60), "in 59m");
        assert_eq!(after(1), "in 1m");
        assert_eq!(after(0), "in 0m");
        assert_eq!(ago_until(now - 5000, now), "in 0m");
        assert_eq!(after(47 * 3600 + 59 * 60), "in 47h 59m");
        assert_eq!(after(48 * 3600), "in 2d");
        assert_eq!(after(48 * 3600 + 1), "in 3d");
    }
}

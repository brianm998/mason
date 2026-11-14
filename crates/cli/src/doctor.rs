// SPDX-License-Identifier: GPL-3.0-or-later

//! `mason doctor` (basic): check the machine and each project's configuration
//! and say what is wrong, in plain words. Never changes anything.

use mason_daemon::config::{load_projects, ProjectCfg};
use mason_landing::git;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Ok,
    /// Worth knowing, not wrong: never counts as a failure.
    Note,
    Warn,
    Fail,
}

pub struct Finding {
    pub level: Level,
    pub text: String,
}

fn ok(t: impl Into<String>) -> Finding {
    Finding {
        level: Level::Ok,
        text: t.into(),
    }
}
fn note(t: impl Into<String>) -> Finding {
    Finding {
        level: Level::Note,
        text: t.into(),
    }
}
fn warn(t: impl Into<String>) -> Finding {
    Finding {
        level: Level::Warn,
        text: t.into(),
    }
}
fn fail(t: impl Into<String>) -> Finding {
    Finding {
        level: Level::Fail,
        text: t.into(),
    }
}

/// Is the `mason` found first on PATH this program? Another tool's `mason` (Dart's, for one)
/// would answer `mason task add` with something else entirely, so say which it is and how to
/// choose. The identity is asked of the binary itself, never guessed from its path.
pub fn check_path_command(out: &mut Vec<Finding>) {
    let exe = std::env::current_exe()
        .ok()
        .and_then(|e| e.canonicalize().ok());
    let Some(found) = on_path("mason") else {
        out.push(warn("`mason` is not on PATH, so typing `mason` finds nothing. Add this program's folder to PATH, or reinstall with the install script or Homebrew"));
        return;
    };
    if found.canonicalize().ok() == exe {
        out.push(ok(format!(
            "`mason` on PATH is this program ({})",
            found.display()
        )));
        return;
    }
    let help = std::process::Command::new(&found)
        .arg("--help")
        .output()
        .ok();
    let first = help
        .as_ref()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .next()
                .unwrap_or_default()
                .trim()
                .to_string()
        })
        .unwrap_or_default();
    if first.starts_with("mason: command-line client for the MASON pipeline") {
        out.push(warn(format!("another copy of mason-pipeline is first on PATH: {}. This one is {}; put the one you want first on PATH", found.display(), exe.map(|e| e.display().to_string()).unwrap_or_default())));
    } else {
        out.push(warn(format!(
            "the `mason` on PATH ({}) is not mason-pipeline{}. Typing `mason` runs that one. Either put {} before it on PATH, or call this one by its full path or an alias (`alias mason-pipeline=...`); for Dart's, `dart pub global deactivate mason_cli` removes it",
            found.display(),
            if first.is_empty() { String::new() } else { format!(" (it says: {first:?})") },
            exe.and_then(|e| e.parent().map(|p| p.display().to_string())).unwrap_or_default()
        )));
    }
}

/// Is some launchd agent (or systemd user unit) running `mason-watchdog.sh`? Only a note when
/// not: the watchdog is optional, but it is the one thing that can say the daemon is dead.
pub fn check_watchdog(dirs: &[PathBuf], out: &mut Vec<Finding>) {
    let found = dirs.iter().any(|d| {
        std::fs::read_dir(d)
            .into_iter()
            .flatten()
            .flatten()
            .any(|e| {
                std::fs::read_to_string(e.path()).is_ok_and(|t| t.contains("mason-watchdog.sh"))
            })
    });
    if found {
        out.push(ok("the watchdog (mason-watchdog.sh) is installed"));
    } else {
        out.push(note("the watchdog is not installed: if masond dies or hangs with no monitor open, nobody is told. See \"The watchdog\" in docs/self-hosting.md"));
    }
}

/// The shell project commands run under (`sh`; on Windows, Git Bash). Missing on Windows is a
/// failure: no step, hook or push can run without it.
pub fn check_shell(found: Result<String, String>, out: &mut Vec<Finding>) {
    match found {
        Ok(t) => out.push(ok(t)),
        Err(e) => out.push(fail(e)),
    }
}

/// Is there a unit written by `mason service install`?
pub fn check_service(dirs: &[PathBuf], out: &mut Vec<Finding>) {
    let found = dirs.iter().find_map(|d| {
        std::fs::read_dir(d)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .find(|p| {
                p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                    n == crate::service::UNIT || n.starts_with(crate::service::LABEL)
                }) && crate::service::is_ours(p)
            })
    });
    match found {
        Some(p) => out.push(ok(format!("masond runs as a service (unit {} written by `mason service install`)", p.display()))),
        None => out.push(note("masond is not installed as a service: `mason service install` makes it start at login and restart if it dies")),
    }
}

fn watchdog_dirs() -> Vec<PathBuf> {
    let Some(h) = std::env::var_os("HOME").map(PathBuf::from) else {
        return vec![];
    };
    vec![
        h.join("Library/LaunchAgents"),
        h.join(".config/systemd/user"),
    ]
}

fn on_path(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|p| {
        std::env::split_paths(&p)
            .map(|d| d.join(name))
            .find(|c| c.is_file())
    })
}

/// The longest socket path a step host could need under `data_dir`.
fn longest_socket(data_dir: &Path) -> usize {
    data_dir
        .join("h")
        .join("t999999r999999-99")
        .join("sock")
        .as_os_str()
        .len()
}

pub use mason_daemon::ssh_keys::github_repo;

fn ago(ms: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |t| t.as_millis() as u64);
    let s = now.saturating_sub(ms) / 1000;
    match s {
        0..=89 => format!("{s}s ago"),
        90..=5399 => format!("{}m ago", s / 60),
        _ => format!("{}h ago", s / 3600),
    }
}

/// The tracker: absent with a GitHub remote is a note; present, what the last
/// look found (from the running daemon, which is the only one that knows).
/// Which issues the tracker imports, as the project's `require_label` says.
fn import_rule(t: &mason_daemon::config::TrackerCfg) -> String {
    if t.require_label {
        format!("imports only issues labelled `{}`", t.label)
    } else {
        "imports the owner's issues and outsiders' labelled ones".into()
    }
}

/// Can MASON reach the project's remote the way masond does (never asking, with the repository's
/// own `core.sshCommand`)? A refusal names the account the host saw.
fn check_remote_access(p: &ProjectCfg, out: &mut Vec<Finding>) {
    let Some(remote) = p.remote.as_deref() else {
        return;
    };
    let name = &p.id;
    match git::run(&p.path, &["ls-remote", remote, "HEAD"]) {
        Ok(_) => out.push(ok(format!("project {name}: the remote {remote} accepts our credentials"))),
        Err(e) => match git::auth_failure(&e.stderr) {
            Some(a) => out.push(fail(format!(
                "project {name}: the remote refuses us{}. Pushes would fail and landing would be held; run `mason ssh-key list --project {name}` and `mason ssh-key use <path>`",
                a.seen_as.map_or(String::new(), |u| format!(": GitHub saw the account {u}"))
            ))),
            None => out.push(warn(format!(
                "project {name}: could not reach the remote {remote}: {}",
                e.stderr.lines().next().unwrap_or("").trim()
            ))),
        },
    }
}

fn check_tracker(p: &ProjectCfg, status: Option<&serde_json::Value>, out: &mut Vec<Finding>) {
    let name = &p.id;
    let Some(t) = &p.services.tracker else {
        let remote = p.remote.as_deref().unwrap_or("origin");
        if let Some(repo) = git::run(&p.path, &["remote", "get-url", remote])
            .ok()
            .and_then(|u| github_repo(&u))
        {
            out.push(note(format!(
                "project {name}: issues on {repo} are not imported; see docs/self-hosting.md"
            )));
        }
        return;
    };
    let Some(s) = status else {
        out.push(ok(format!(
            "project {name}: tracker {} configured, {}; the poll state is unknown while masond is not running",
            t.repo,
            import_rule(t)
        )));
        return;
    };
    let Some(v) = s["trackers"]
        .as_array()
        .and_then(|a| a.iter().find(|v| v["project"] == name.as_str()))
    else {
        out.push(warn(format!(
            "project {name}: tracker {} is configured but masond does not report it (restart masond to read the changed config)",
            t.repo
        )));
        return;
    };
    if v["state"] == "stopped" {
        out.push(warn(format!(
            "project {name}: the tracker poll for {} stopped: {}",
            t.repo,
            v["reason"].as_str().unwrap_or("no reason recorded")
        )));
        return;
    }
    let n = |k: &str| v[k].as_u64().unwrap_or(0);
    let mut line = match v["last_poll_ms"].as_u64() {
        Some(ms) => format!(
            "project {name}: tracker {} last polled {}: {} imported, {} ignored, {} unchanged, {} waiting for a slot; {}",
            t.repo, ago(ms), n("imported"), n("ignored"), n("unchanged"), n("waiting_slot"), import_rule(t)
        ),
        None => format!(
            "project {name}: tracker {} has not been polled yet; {}",
            t.repo,
            import_rule(t)
        ),
    };
    if let Some(e) = v["error"].as_str() {
        line.push_str(&format!("; the last look failed: {e}"));
        out.push(warn(line));
    } else {
        out.push(ok(line));
    }
}

fn check_project(p: &ProjectCfg, out: &mut Vec<Finding>) {
    let name = &p.id;
    if !p.path.join(".git").exists() && !p.path.is_dir() {
        out.push(fail(format!(
            "project {name}: {} does not exist",
            p.path.display()
        )));
        return;
    }
    if git::run(&p.path, &["rev-parse", "--git-dir"]).is_err() {
        out.push(fail(format!(
            "project {name}: {} is not a git repository",
            p.path.display()
        )));
        return;
    }
    out.push(ok(format!(
        "project {name}: {} is a git repository",
        p.path.display()
    )));
    if let Some(r) = &p.remote {
        if git::run(&p.path, &["remote", "get-url", r]).is_err() {
            out.push(fail(format!(
                "project {name}: the remote {r:?} is not configured in the repository"
            )));
        } else {
            out.push(ok(format!("project {name}: remote {r:?} is configured")));
        }
    } else {
        out.push(warn(format!("project {name}: no remote; `direct` landing needs one (or the base branch checked out nowhere)")));
    }
    let base = p.base_ref();
    if git::rev_parse(&p.path, &base).is_err() {
        let hint = if p.remote.is_some() {
            " (try `git fetch`)"
        } else {
            ""
        };
        out.push(fail(format!(
            "project {name}: the base ref {base:?} does not exist{hint}"
        )));
    } else {
        out.push(ok(format!("project {name}: base ref {base:?} exists")));
    }
    // The pipeline definition on the base branch, if the project has one.
    let path = format!("{base}:.mason/mason.toml");
    // The step types the base branch's extensions provide (read from the commit, never run).
    let caps = mason_definition::Capabilities::now().with_plugins(
        git::rev_parse(&p.path, &base)
            .map(|sha| mason_daemon::plugins::scan_commit(&p.path, &sha).types())
            .unwrap_or_default(),
    );
    if git::ok(&p.path, &["cat-file", "-e", &path]) {
        match git::run(&p.path, &["show", &path]) {
            Ok(text) => match mason_definition::parse_with(
                &text,
                mason_definition::Plugins::Named(&caps.plugin_names()),
            ) {
                Err(ps) => ps.iter().for_each(|x| {
                    out.push(fail(format!(
                        "project {name}: .mason/mason.toml on {base}: {x}"
                    )))
                }),
                Ok(def) => {
                    let problems = mason_definition::validate(&def, &caps);
                    if problems.is_empty() {
                        out.push(ok(format!(
                            "project {name}: .mason/mason.toml on {base} is valid ({} steps)",
                            def.steps.len()
                        )));
                    } else {
                        problems.iter().for_each(|x| {
                            out.push(fail(format!(
                                "project {name}: .mason/mason.toml on {base}: {x}"
                            )))
                        });
                    }
                }
            },
            Err(e) => out.push(fail(format!(
                "project {name}: cannot read .mason/mason.toml on {base}: {e}"
            ))),
        }
    } else {
        out.push(ok(format!(
            "project {name}: no .mason/mason.toml on {base}, so the built-in pipeline is used"
        )));
    }
    check_landing(p, &base, out);
    match &p.claude_bin {
        Some(b) if b.is_file() => out.push(ok(format!("project {name}: agent binary {}", b.display()))),
        Some(b) => out.push(fail(format!("project {name}: claude_bin {} does not exist", b.display()))),
        None => match on_path("claude") {
            Some(b) => out.push(ok(format!("project {name}: agent binary {}", b.display()))),
            None => out.push(warn(format!("project {name}: `claude` is not on PATH and no claude_bin is set; agent steps cannot run"))),
        },
    }
}

/// `.mason/landing.toml` on the base: it parses, the mode has what it needs, and
/// where CI is GitHub Actions a workflow can run on a candidate ref.
fn check_landing(p: &ProjectCfg, base: &str, out: &mut Vec<Finding>) {
    let name = &p.id;
    let spec = format!("{base}:.mason/landing.toml");
    let text = if git::ok(&p.path, &["cat-file", "-e", &spec]) {
        git::run(&p.path, &["show", &spec]).unwrap_or_default()
    } else {
        String::new()
    };
    let cfg = match mason_landing::config::parse(&text) {
        Ok(c) => c,
        Err(ps) => {
            for x in ps {
                out.push(fail(format!(
                    "project {name}: .mason/landing.toml on {base}: {x}"
                )));
            }
            return;
        }
    };
    let needs = mason_landing::config::service_problems(
        &cfg,
        p.remote.is_some(),
        p.services.host.is_some(),
        p.services.ci.is_some(),
    );
    if needs.is_empty() {
        out.push(ok(format!(
            "project {name}: landing mode `{}` has what it needs",
            cfg.mode.name()
        )));
    }
    for x in needs {
        out.push(fail(format!("project {name}: {x}")));
    }
    if (cfg.mode.uses_candidate_ref() || cfg.mode.uses_pr()) && cfg.batch_suites().is_none() {
        out.push(warn(format!(
            "project {name}: landing mode `{}` has no `[[tier]] name = \"batch\"`, so the batch union gets no local verification and a defect that exists only in the combination reaches CI",
            cfg.mode.name()
        )));
    }
    let github_ci = p
        .services
        .ci
        .as_ref()
        .is_some_and(|c| matches!(c.kind, mason_daemon::config::CiKind::GithubActions { .. }));
    if github_ci && cfg.mode.uses_candidate_ref() {
        let problems = mason_adapters::github::workflow_problems_at(&p.path, base);
        if problems.is_empty() {
            out.push(ok(format!(
                "project {name}: a workflow runs on `mason/ci/**` or by dispatch"
            )));
        }
        for x in problems {
            out.push(fail(format!("project {name}: {}", x.0)));
        }
    }
}

/// The effective tier mapping of the project's provider, entry by entry with
/// where each came from, and anything wrong with the files behind it.
fn check_providers(p: &ProjectCfg, config_dir: &Path, out: &mut Vec<Finding>) {
    use mason_daemon::tiermap as providers;
    let provider = p.agent().provider;
    let e = providers::effective(config_dir, provider, &p.overlay_text);
    for problem in &e.problems {
        out.push(warn(format!("project {}: {problem}", p.id)));
    }
    out.push(ok(format!(
        "project {}: provider {} maps tiers and levels as follows",
        p.id,
        providers::name(provider)
    )));
    for (tier, level, entry, source) in e.mapping.rows() {
        out.push(ok(format!(
            "  {:<5} {:<6} -> model {:<12} effort {:<8} ({})",
            tier.name(),
            level.name(),
            entry.model.as_deref().unwrap_or("CLI default"),
            entry.effort.as_deref().unwrap_or("none"),
            source.label()
        )));
    }
    // Definition warnings, from the base branch as the daemon reads it.
    let at = format!("{}:.mason/mason.toml", p.base_ref());
    if let Ok(text) = git::run(&p.path, &["show", &at]) {
        let caps =
            mason_definition::Capabilities::now().with_efforts(providers::effort_levels(provider));
        if let Ok(def) = mason_definition::parse(&text) {
            for w in mason_definition::validate::warnings(&def, &caps) {
                out.push(warn(format!("project {}: {w}", p.id)));
            }
        }
    }
}

/// One line per provider account: its adapter's CLI, its billing kind and the
/// projects that use it. Login is not probed: the adapters learn it only when
/// a step runs (the CLI's "Not logged in" result), so it is said so, not guessed.
fn check_accounts(
    accounts: &[mason_daemon::providers::Account],
    projects: &[ProjectCfg],
    out: &mut Vec<Finding>,
) {
    use mason_daemon::config::Provider;
    use mason_daemon::providers::adapter_name;
    for a in accounts {
        let users: Vec<&ProjectCfg> = projects.iter().filter(|p| p.account == a.name).collect();
        let cli = adapter_name(a.adapter);
        let set = users.iter().find_map(|p| match a.adapter {
            Provider::Claude => p.claude_bin.clone(),
            Provider::Codex => p.agent().codex_bin,
        });
        let found = match set {
            Some(b) if b.is_file() => Ok(b),
            Some(b) => Err(format!(
                "{} (set for a project) does not exist",
                b.display()
            )),
            None => on_path(cli).ok_or_else(|| format!("`{cli}` is not on PATH")),
        };
        let names = if users.is_empty() {
            "no projects".to_string()
        } else {
            format!(
                "projects {}",
                users
                    .iter()
                    .map(|p| p.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        let head = format!(
            "account {}: adapter {cli}, billing {}, {names}",
            a.name,
            a.billing.name()
        );
        match found {
            Ok(b) => out.push(ok(format!("{head}; CLI {}", b.display()))),
            Err(e) => out.push(warn(format!("{head}; CLI not found: {e}"))),
        }
        out.push(ok(format!(
            "account {}: login is not checked here; a step that finds the {cli} CLI logged out stops with \"Not logged in\"",
            a.name
        )));
    }
}

/// Warn when no channel reaches the owner with no monitor open.
pub fn check_notify(config_dir: &Path, out: &mut Vec<Finding>) {
    let settings = match std::fs::read_to_string(config_dir.join("notify.toml")) {
        Ok(t) => mason_daemon::notify::parse(&t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Default::default()),
        Err(e) => Err(format!("notify.toml: {e}")),
    };
    match settings {
        Ok(s) if s.reaches_without_monitor() => out.push(ok("an alert can reach you with no monitor open")),
        Ok(_) => out.push(warn(format!(
            "{} Add to {}/notify.toml:\n    [speech]\n    daemon = true\n    command = [\"say\"]\n  or, for a phone:\n    [ntfy]\n    topic = \"<a topic only you know>\"",
            mason_daemon::notify::NO_REACH_LINE,
            config_dir.display()
        ))),
        Err(e) => out.push(fail(e)),
    }
}

pub fn run(data_dir: &Path, config_dir: &Path) -> Vec<Finding> {
    let mut out = vec![];
    match std::process::Command::new("git").arg("--version").output() {
        Ok(o) if o.status.success() => {
            out.push(ok(String::from_utf8_lossy(&o.stdout).trim().to_string()))
        }
        _ => out.push(fail("git is not installed or not on PATH")),
    }

    if std::fs::create_dir_all(data_dir).is_ok()
        && std::fs::write(data_dir.join(".doctor"), "x").is_ok()
    {
        let _ = std::fs::remove_file(data_dir.join(".doctor"));
        out.push(ok(format!(
            "data directory {} is writable",
            data_dir.display()
        )));
    } else {
        out.push(fail(format!(
            "cannot write to the data directory {}",
            data_dir.display()
        )));
    }
    let longest = longest_socket(data_dir);
    if longest > 100 {
        out.push(fail(format!("the data directory path is too long: step host sockets would need {longest} bytes, and the limit is about 100. Use a shorter --data-dir")));
    } else {
        out.push(ok(format!(
            "socket paths fit ({longest} of about 100 bytes)"
        )));
    }

    check_path_command(&mut out);
    check_shell(mason_platform::shell::describe(), &mut out);
    check_watchdog(&watchdog_dirs(), &mut out);
    check_service(&watchdog_dirs(), &mut out);
    let exe = std::env::current_exe().unwrap_or_default();
    for b in ["masond", "step-host", "mason-mcp"] {
        let p = exe.with_file_name(b);
        if p.is_file() {
            out.push(ok(format!("{b} found next to mason")));
        } else {
            out.push(warn(format!(
                "{b} not found next to mason ({}); pass its path to masond explicitly",
                p.display()
            )));
        }
    }

    match mason_daemon::providers::load(config_dir) {
        Ok(accounts) => {
            let ps = load_projects(config_dir).unwrap_or_default();
            check_accounts(&accounts, &ps, &mut out);
        }
        Err(e) => out.push(fail(e)),
    }
    match load_projects(config_dir) {
        Ok(ps) if ps.is_empty() => out.push(warn(format!(
            "no projects configured: add one as {}/projects/<id>.toml",
            config_dir.display()
        ))),
        Ok(ps) => {
            let live = crate::client::find(data_dir)
                .ok()
                .and_then(|d| d.call("GET", "/status", None).ok());
            for p in &ps {
                check_project(p, &mut out);
                check_providers(p, config_dir, &mut out);
                check_tracker(p, live.as_ref(), &mut out);
                check_remote_access(p, &mut out);
            }
        }
        Err(errs) => errs
            .into_iter()
            .for_each(|e| out.push(fail(format!("project config: {e}")))),
    }

    check_notify(config_dir, &mut out);

    match crate::client::find(data_dir) {
        Ok(d) => match d.call("GET", "/status", None) {
            Ok(s) => {
                out.push(ok(format!(
                    "masond is running (pid {}, {} step hosts)",
                    s["pid"], s["step_hosts_running"]
                )));
                if s["event_loop_stalls"].as_u64().unwrap_or(0) > 0 {
                    out.push(warn(format!(
                        "the event loop stalled {} time(s); worst {} ms",
                        s["event_loop_stalls"], s["worst_stall_ms"]
                    )));
                }
                if let Some(n) = s["thread_panics"]["count"].as_u64().filter(|n| *n > 0) {
                    out.push(warn(format!(
                        "{n} daemon thread panic(s) were caught and the thread went on; last: {}",
                        s["thread_panics"]["last"].as_str().unwrap_or("?")
                    )));
                }
                if s["bugs"].as_u64().unwrap_or(0) > 0 {
                    out.push(warn(format!(
                        "the daemon has filed {} bug(s) (see `mason status`)",
                        s["bugs"]
                    )));
                }
            }
            Err(e) => out.push(fail(format!("masond did not answer: {e}"))),
        },
        Err(e) => out.push(warn(e.to_string())),
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn doctor_reports_the_shell_or_what_to_install() {
        let mut out = vec![];
        super::check_shell(Ok("project commands run under C:/Git/bin/bash.exe".into()), &mut out);
        super::check_shell(Err(mason_platform::shell::MISSING.into()), &mut out);
        assert_eq!(out[0].level, super::Level::Ok);
        assert!(out[0].text.contains("bash.exe"));
        assert_eq!(out[1].level, super::Level::Fail);
        assert!(out[1].text.contains("Git for Windows"));
    }

    use super::*;

    fn repo_with(files: &[(&str, &str)]) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let run = |a: &[&str]| {
            assert!(std::process::Command::new("git")
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(a)
                .current_dir(d.path())
                .status()
                .unwrap()
                .success());
        };
        run(&["init", "-q", "-b", "main"]);
        for (p, t) in files {
            let f = d.path().join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, t).unwrap();
        }
        run(&["add", "-A"]);
        run(&["commit", "-qm", "x"]);
        d
    }

    fn project(repo: &Path, extra: &str) -> ProjectCfg {
        mason_daemon::config::parse_project(
            "p",
            &format!(
                "path = \"{}\"\nbase = \"main\"\ntest = \"true\"\nremote = \"origin\"\n{extra}",
                repo.display()
            ),
        )
        .unwrap()
    }

    const GH_CI: &str =
        "[ci]\nkind = \"github-actions\"\nrepo = \"o/r\"\ntoken = \"env:GH_TOKEN\"\n";

    #[test]
    fn doctor_checks_that_ci_can_run_on_a_candidate_ref() {
        let landing = ".mason/landing.toml";
        for (wf, ok) in [
            ("on:\n  push:\n    branches: [main]\n", false),
            ("on:\n  push:\n    branches: [\"mason/ci/**\"]\n", true),
            ("on:\n  workflow_dispatch:\n", true),
        ] {
            let d = repo_with(&[
                (landing, "mode = \"direct-ci\"\n"),
                (".github/workflows/ci.yml", wf),
            ]);
            let mut out = vec![];
            check_landing(&project(d.path(), GH_CI), "main", &mut out);
            let bad = out
                .iter()
                .filter(|f| matches!(f.level, Level::Fail))
                .map(|f| f.text.clone())
                .collect::<Vec<_>>();
            assert_eq!(bad.is_empty(), ok, "{wf:?}: {bad:?}");
            if !ok {
                assert!(bad[0].contains("mason/ci/**"), "{bad:?}");
            }
        }
        // No workflow at all.
        let d = repo_with(&[(landing, "mode = \"direct-ci\"\n")]);
        let mut out = vec![];
        check_landing(&project(d.path(), GH_CI), "main", &mut out);
        assert!(out
            .iter()
            .any(|f| matches!(f.level, Level::Fail) && f.text.contains("no workflow")));
    }

    #[test]
    fn doctor_names_what_a_landing_mode_lacks_and_reads_landing_toml_errors() {
        let d = repo_with(&[(".mason/landing.toml", "mode = \"pr-batch\"\n")]);
        let mut out = vec![];
        check_landing(&project(d.path(), ""), "main", &mut out);
        assert!(
            out.iter()
                .any(|f| matches!(f.level, Level::Fail) && f.text.contains("[host]")),
            "{:?}",
            out.iter().map(|f| &f.text).collect::<Vec<_>>()
        );
        let d = repo_with(&[(
            ".mason/landing.toml",
            "mode = \"direct\"\n[[tier]]\nname = \"batch\"\nci = \"x\"\n",
        )]);
        let mut out = vec![];
        check_landing(&project(d.path(), ""), "main", &mut out);
        assert!(out
            .iter()
            .any(|f| matches!(f.level, Level::Fail) && f.text.contains("direct-ci")));
        // No landing.toml: the default, direct landing, which needs nothing.
        let d = repo_with(&[("README", "x")]);
        let mut out = vec![];
        check_landing(&project(d.path(), ""), "main", &mut out);
        assert!(out.iter().all(|f| !matches!(f.level, Level::Fail)));
    }

    #[test]
    fn doctor_warns_when_a_ci_or_pr_mode_has_no_batch_tier() {
        let tier = "[[tier]]\nname = \"batch\"\nsuites = [\"a\"]\n";
        let warned = |toml: &str| {
            let d = repo_with(&[(".mason/landing.toml", toml)]);
            let mut out = vec![];
            check_landing(&project(d.path(), ""), "main", &mut out);
            out.iter()
                .any(|f| matches!(f.level, Level::Warn) && f.text.contains("batch"))
        };
        for m in [
            "direct-ci",
            "local-only-ci",
            "pr-each",
            "pr-batch",
            "host-queue",
        ] {
            assert!(warned(&format!("mode = \"{m}\"\n")), "{m}");
            assert!(!warned(&format!("mode = \"{m}\"\n{tier}")), "{m} with tier");
        }
        for m in ["direct", "local-only"] {
            assert!(!warned(&format!("mode = \"{m}\"\n")), "{m}");
        }
    }

    #[test]
    fn star_example_has_a_cheap_batch_tier_the_impact_map_narrows() {
        let cfg = mason_landing::config::parse(include_str!(
            "../../../docs/migrations/star/.mason/landing.toml"
        ))
        .unwrap();
        let suites: Vec<String> = cfg.batch_suites().unwrap().to_vec();
        assert_eq!(suites, ["t0-pipeline-static", "t0-diff-check", "t0-i18n"]);
        let toml = include_str!("../../../docs/migrations/star/.mason/mason.toml");
        let testing = mason_definition::parse_with(toml, mason_definition::Plugins::Any)
            .unwrap_or_else(|e| panic!("{e:?}"))
            .testing;
        let sel = testing
            .impact
            .select(&["pipeline/src/x.py".to_string()], &suites);
        assert!(
            matches!(&sel, mason_testing::Selection::Some(v) if v.iter().any(|s| s == "t0-pipeline-static")),
            "{sel:?}"
        );
        let sel = testing
            .impact
            .select(&["somewhere/new.txt".to_string()], &suites);
        assert!(matches!(sel, mason_testing::Selection::All), "{sel:?}");
    }

    #[test]
    fn doctor_lists_each_account_with_its_billing_and_projects() {
        let d = repo_with(&[("README", "x")]);
        let accts = mason_daemon::providers::parse_providers(
            "[provider.a]\nadapter=\"claude\"\nbilling=\"api\"\n[provider.b]\nadapter=\"codex\"\nbilling=\"subscription\"\n",
        )
        .unwrap();
        let mut p = project(d.path(), "");
        p.account = "b".into();
        let mut out = vec![];
        check_accounts(&accts, &[p], &mut out);
        let t: Vec<String> = out.iter().map(|f| f.text.clone()).collect();
        assert!(
            t.iter()
                .any(|x| x.contains("account a: adapter claude, billing api, no projects")),
            "{t:?}"
        );
        assert!(
            t.iter()
                .any(|x| x.contains("account b: adapter codex, billing subscription, projects p")),
            "{t:?}"
        );
    }

    fn notify_findings(toml: Option<&str>) -> Vec<Finding> {
        let dir = tempfile::tempdir().unwrap();
        if let Some(t) = toml {
            std::fs::write(dir.path().join("notify.toml"), t).unwrap();
        }
        let mut out = vec![];
        check_notify(dir.path(), &mut out);
        out
    }

    #[test]
    fn doctor_warns_when_no_channel_works_with_no_monitor_open() {
        let out = notify_findings(None);
        let w = out
            .iter()
            .find(|f| f.level == Level::Warn)
            .expect("a warning");
        assert!(w.text.contains("daemon = true") && w.text.contains("command = [\"say\"]"));
        assert!(w.text.contains("[ntfy]") && w.text.contains("topic"));
        let speech_off = notify_findings(Some("[speech]\ndaemon = true\n"));
        assert!(speech_off.iter().any(|f| f.level == Level::Warn));
    }

    #[test]
    fn doctor_is_quiet_with_push_or_daemon_speech() {
        for t in [
            "[ntfy]\ntopic = \"t\"\n",
            "[speech]\ndaemon = true\ncommand = [\"say\"]\n",
        ] {
            assert!(
                notify_findings(Some(t))
                    .iter()
                    .all(|f| f.level != Level::Warn),
                "{t}"
            );
        }
    }

    #[test]
    fn the_watchdog_is_noted_when_not_installed() {
        let d = tempfile::tempdir().unwrap();
        let mut out = vec![];
        check_watchdog(&[d.path().to_path_buf()], &mut out);
        assert!(matches!(out[0].level, Level::Note), "{}", out[0].text);
        std::fs::write(
            d.path().join("w.plist"),
            "<string>/x/scripts/mason-watchdog.sh</string>",
        )
        .unwrap();
        let mut out = vec![];
        check_watchdog(&[d.path().to_path_buf()], &mut out);
        assert!(matches!(out[0].level, Level::Ok));
    }

    #[test]
    fn github_remotes_are_recognised() {
        for (u, want) in [
            ("https://github.com/o/r.git", Some("o/r")),
            ("https://tok@github.com/o/r", Some("o/r")),
            ("git@github.com:o/r.git\n", Some("o/r")),
            ("ssh://git@github.com/o/r.git", Some("o/r")),
            ("git@gitlab.com:o/r.git", None),
            ("/srv/git/o/r.git", None),
            ("https://github.com/o", None),
        ] {
            assert_eq!(github_repo(u).as_deref(), want, "{u}");
        }
    }

    #[test]
    fn a_github_remote_without_a_tracker_is_a_note_not_a_failure() {
        let d = repo_with(&[("README", "x")]);
        let g = |a: &[&str]| {
            assert!(std::process::Command::new("git")
                .args(a)
                .current_dir(d.path())
                .status()
                .unwrap()
                .success())
        };
        g(&["remote", "add", "origin", "git@github.com:me/thing.git"]);
        let mut out = vec![];
        check_tracker(&project(d.path(), ""), None, &mut out);
        assert_eq!(out.len(), 1);
        assert!(matches!(out[0].level, Level::Note));
        assert_eq!(
            out[0].text,
            "project p: issues on me/thing are not imported; see docs/self-hosting.md"
        );
        // With a tracker the note is gone and the counts come from the daemon.
        let tr = "[tracker]\nkind = \"github\"\nrepo = \"me/thing\"\ntoken = \"env:T\"\n";
        let st = serde_json::json!({"trackers": [{"project": "p", "state": "polling",
            "last_poll_ms": 1, "imported": 3, "ignored": 2, "unchanged": 1, "waiting_slot": 4}]});
        let mut out = vec![];
        check_tracker(&project(d.path(), tr), Some(&st), &mut out);
        assert!(
            out[0]
                .text
                .contains("3 imported, 2 ignored, 1 unchanged, 4 waiting for a slot"),
            "{}",
            out[0].text
        );
        let st = serde_json::json!({"trackers": [{"project": "p", "state": "stopped", "reason": "404"}]});
        let mut out = vec![];
        check_tracker(&project(d.path(), tr), Some(&st), &mut out);
        assert!(matches!(out[0].level, Level::Warn) && out[0].text.contains("stopped: 404"));
    }

    #[test]
    fn the_tracker_line_names_the_import_rule_in_force() {
        let d = repo_with(&[("README", "x")]);
        let st = serde_json::json!({"trackers": [{"project": "p", "state": "polling",
            "last_poll_ms": 1, "imported": 0, "ignored": 0, "unchanged": 0, "waiting_slot": 0}]});
        let head = "[tracker]\nkind = \"github\"\nrepo = \"me/thing\"\ntoken = \"env:T\"\nlabel = \"mason\"\n";
        let text = |extra: &str| {
            let mut out = vec![];
            check_tracker(
                &project(d.path(), &format!("{head}{extra}")),
                Some(&st),
                &mut out,
            );
            out[0].text.clone()
        };
        let on = text("require_label = true\n");
        assert!(on.contains("imports only issues labelled `mason`"), "{on}");
        let off = text("");
        assert!(
            off.contains("imports the owner's issues and outsiders' labelled ones"),
            "{off}"
        );
        let mut out = vec![];
        check_tracker(&project(d.path(), head), None, &mut out);
        assert!(
            out[0].text.contains("outsiders' labelled ones"),
            "{}",
            out[0].text
        );
    }

    #[test]
    fn long_data_directories_are_caught() {
        assert!(longest_socket(Path::new("/tmp/x")) < 100);
        let long = format!("/tmp/{}", "a".repeat(90));
        assert!(longest_socket(Path::new(&long)) > 100);
        let f = run(Path::new(&long), Path::new("/nonexistent"));
        assert!(f
            .iter()
            .any(|f| matches!(f.level, Level::Fail) && f.text.contains("too long")));
    }
}

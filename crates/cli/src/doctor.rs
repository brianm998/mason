// SPDX-License-Identifier: GPL-3.0-or-later

//! `mason doctor` (basic): check the machine and each project's configuration
//! and say what is wrong, in plain words. Never changes anything.

use mason_daemon::config::{load_projects, ProjectCfg};
use mason_landing::git;
use std::path::{Path, PathBuf};

pub enum Level {
    Ok,
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
    if git::ok(&p.path, &["cat-file", "-e", &path]) {
        match git::run(&p.path, &["show", &path]) {
            Ok(text) => match mason_definition::parse(&text) {
                Err(ps) => ps.iter().for_each(|x| {
                    out.push(fail(format!(
                        "project {name}: .mason/mason.toml on {base}: {x}"
                    )))
                }),
                Ok(def) => {
                    let problems =
                        mason_definition::validate(&def, &mason_definition::Capabilities::now());
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
    match &p.claude_bin {
        Some(b) if b.is_file() => out.push(ok(format!("project {name}: agent binary {}", b.display()))),
        Some(b) => out.push(fail(format!("project {name}: claude_bin {} does not exist", b.display()))),
        None => match on_path("claude") {
            Some(b) => out.push(ok(format!("project {name}: agent binary {}", b.display()))),
            None => out.push(warn(format!("project {name}: `claude` is not on PATH and no claude_bin is set; agent steps cannot run"))),
        },
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

    match load_projects(config_dir) {
        Ok(ps) if ps.is_empty() => out.push(warn(format!(
            "no projects configured: add one as {}/projects/<id>.toml",
            config_dir.display()
        ))),
        Ok(ps) => ps.iter().for_each(|p| check_project(p, &mut out)),
        Err(errs) => errs
            .into_iter()
            .for_each(|e| out.push(fail(format!("project config: {e}")))),
    }

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
                if s["bugs"].as_u64().unwrap_or(0) > 0 {
                    out.push(warn(format!("the daemon's reconcile sweep has filed {} bug(s): it found things the event path missed", s["bugs"])));
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
    use super::*;

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

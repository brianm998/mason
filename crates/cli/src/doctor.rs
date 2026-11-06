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

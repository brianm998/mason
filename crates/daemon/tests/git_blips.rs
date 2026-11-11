// SPDX-License-Identifier: GPL-3.0-or-later

//! A brief git failure decides nothing. A stub `git` early on `PATH` fails a chosen
//! subcommand; the step goes on from the last-known base, a batch does not start on a
//! configuration it could not read, the definition in force stays, and a lock error is
//! waited out.

mod support;
use mason_core::*;
use mason_daemon::defs::Reload;
use mason_daemon::landing::{read_config, ConfigError};
use serde_json::json;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;
use support::*;

struct Stub {
    dir: PathBuf,
    _tmp: tempfile::TempDir,
}

static SERIAL: Mutex<()> = Mutex::new(());
static STUB: OnceLock<Stub> = OnceLock::new();

/// Installs the stub once for this process and serialises the tests (the stub and `PATH`
/// are process-wide).
fn stub() -> (MutexGuard<'static, ()>, &'static Stub) {
    let g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let s = STUB.get_or_init(|| {
        let real = String::from_utf8(
            std::process::Command::new("sh")
                .args(["-c", "command -v git"])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();
        let tmp = tempfile::Builder::new()
            .prefix("gs")
            .tempdir_in("/tmp")
            .unwrap();
        let dir = std::fs::canonicalize(tmp.path()).unwrap();
        let script = format!(
            r#"#!/bin/sh
f="{dir}/fail"
if [ -f "$f" ]; then
  {{ read mode; read sub; read pat; read msg; }} < "$f"
  hit=0
  for a in "$@"; do [ "$a" = "$sub" ] && hit=1; done
  if [ $hit = 1 ]; then
    case "$*" in *"$pat"*) ;; *) [ "$pat" = "-" ] || hit=0 ;; esac
  fi
  if [ $hit = 1 ]; then
    [ "$mode" = once ] && rm -f "$f"
    echo "$msg" >&2
    exit 128
  fi
fi
exec {real} "$@"
"#,
            dir = dir.display(),
        );
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        let git = dir.join("bin/git");
        std::fs::write(&git, script).unwrap();
        std::process::Command::new("chmod")
            .arg("+x")
            .arg(&git)
            .status()
            .unwrap();
        let path = std::env::var("PATH").unwrap_or_default();
        std::env::set_var("PATH", format!("{}:{path}", dir.join("bin").display()));
        Stub { dir, _tmp: tmp }
    });
    s.clear();
    (g, s)
}

impl Stub {
    /// Fail `sub` (when the arguments also contain `pat`, or `-` for any) with `msg`:
    /// `once`, or `always` until [`Stub::clear`].
    fn fail(&self, mode: &str, sub: &str, pat: &str, msg: &str) {
        std::fs::write(self.dir.join("fail"), format!("{mode}\n{sub}\n{pat}\n{msg}\n")).unwrap();
    }
    fn clear(&self) {
        let _ = std::fs::remove_file(self.dir.join("fail"));
    }
}

fn events(d: &std::sync::Arc<mason_daemon::Daemon>) -> String {
    d.store
        .read_since(0, 10_000)
        .unwrap()
        .iter()
        .map(|e| format!("{:?}", e.event.kind))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn a_missing_landing_toml_is_the_defaults_and_an_unreadable_one_is_no_answer() {
    let (_g, stub) = stub();
    let s = Scratch::new("true", json!({"steps": {}}));
    let d = s.start();
    let pid = d.project_id_by_name("scratch").unwrap();
    // No file on the base: the defaults, as before.
    let c = read_config(&d, pid).unwrap();
    assert_eq!(c.mode, mason_landing::config::Mode::Direct);

    // A file there is read.
    s.push_files(&[(".mason/landing.toml", "window = \"10m\"\n")], "landing");
    mason_landing::git::run(&s.repo, &["fetch", "-q", "origin"]).unwrap();
    assert_eq!(
        read_config(&d, pid).unwrap().window,
        Duration::from_secs(600)
    );

    // Git failing is not "no file": no answer, never the defaults.
    stub.fail("always", "ls-tree", "landing.toml", "fatal: Unable to create 'x/index.lock'");
    let e = read_config(&d, pid).unwrap_err();
    assert!(matches!(e, ConfigError::Unread(_)), "{e:?}");
    stub.fail("always", "show", "landing.toml", "fatal: broken");
    assert!(matches!(
        read_config(&d, pid).unwrap_err(),
        ConfigError::Unread(_)
    ));
    stub.clear();
    assert_eq!(
        read_config(&d, pid).unwrap().window,
        Duration::from_secs(600)
    );
}

#[test]
fn a_step_goes_on_from_the_last_known_base_after_a_failed_fetch() {
    let (_g, stub) = stub();
    let s = Scratch::new("test -f hello.txt", happy_script());
    let d = s.start();
    stub.fail("always", "fetch", "-", "fatal: unable to access remote: could not resolve host");
    let t = file_task(&d, "fetch fails");
    wait_for("the develop step to start", 60, || {
        steps_run(&d, t).iter().any(|n| n == "develop")
    });
    assert_ne!(state_of(&d, t), State::Failed, "{}", events(&d));
    stub.clear();
    wait_state(&d, t, State::Landed, 120);
}

#[test]
fn a_batch_does_not_start_on_a_configuration_it_could_not_read() {
    let (_g, stub) = stub();
    let s = Scratch::new("test -f hello.txt", happy_script());
    let d = s.start();
    stub.fail("always", "ls-tree", "landing.toml", "fatal: could not read");
    let t = file_task(&d, "config unreadable");
    wait_for("the landing step", 60, || {
        steps_run(&d, t).iter().any(|n| n == "land")
    });
    std::thread::sleep(Duration::from_secs(4));
    // Held: neither landed on the defaults nor failed.
    assert_eq!(state_of(&d, t), State::Landing, "{}", events(&d));
    stub.clear();
    wait_state(&d, t, State::Landed, 60);
}

const DEF_LINT: &str = r#"schema = 1
start = "develop"

[step.develop]
type = "agent"
category = "developing"
next = "test"

[step.test]
type = "test"
category = "testing"
max_rounds = 2
on_red = "develop"
next = "land"

[step.land]
type = "landing"
category = "landing"
"#;

#[test]
fn the_definition_in_force_does_not_change_on_a_failed_git_read() {
    let (_g, stub) = stub();
    let s = Scratch::new("test -f hello.txt", happy_script());
    s.push_files(&[(".mason/mason.toml", DEF_LINT)], "definition");
    let d = s.start();
    let pid = d.project_id_by_name("scratch").unwrap();
    let before = d.current_def(pid).hash.clone();
    assert!(matches!(
        d.current_origin(pid),
        mason_daemon::defs::Origin::Project { .. }
    ));

    stub.fail("always", "rev-parse", "-", "fatal: bad object");
    assert!(matches!(d.reload_definition(pid), Reload::Refused(_)));
    assert_eq!(d.current_def(pid).hash, before);
    assert!(matches!(
        d.current_origin(pid),
        mason_daemon::defs::Origin::Project { .. }
    ));
    assert!(d.definition_error(pid).is_some());

    stub.fail("always", "ls-tree", "mason.toml", "fatal: bad object");
    assert!(matches!(d.reload_definition(pid), Reload::Refused(_)));
    assert_eq!(d.current_def(pid).hash, before);

    stub.clear();
    assert_eq!(d.reload_definition(pid), Reload::Unchanged);
    assert_eq!(d.current_def(pid).hash, before);
    assert!(d.definition_error(pid).is_none());
}

#[test]
fn a_lock_error_in_prepare_waits_and_the_step_goes_on_when_it_clears() {
    let (_g, stub) = stub();
    let s = Scratch::new("test -f hello.txt", happy_script());
    let d = s.start();
    stub.fail(
        "once",
        "worktree",
        "add",
        "fatal: Unable to create '/x/.git/index.lock': File exists.",
    );
    let t = file_task(&d, "lock");
    wait_state(&d, t, State::Landed, 120);
    let ev = events(&d);
    assert!(ev.contains("git is busy"), "{ev}");
}

#[test]
fn a_lock_error_that_lasts_fails_the_step_naming_what_failed() {
    let (_g, stub) = stub();
    mason_daemon::jobs::set_env_wait(Duration::from_secs(2), Duration::from_millis(200));
    let s = Scratch::new("test -f hello.txt", happy_script());
    let d = s.start();
    stub.fail(
        "always",
        "worktree",
        "add",
        "fatal: Unable to create '/x/.git/packed-refs.lock': File exists.",
    );
    let t = file_task(&d, "lock lasts");
    wait_for("the task to fail", 60, || state_of(&d, t) == State::Failed);
    let ev = events(&d);
    mason_daemon::jobs::set_env_wait(Duration::ZERO, Duration::ZERO);
    assert!(ev.contains("preparing the worktree") && ev.contains("packed-refs.lock"), "{ev}");
}

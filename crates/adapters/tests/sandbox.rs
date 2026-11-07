// SPDX-License-Identifier: GPL-3.0-or-later

//! Sandbox wrappers: argv and profile text are checked everywhere; a live run
//! happens only when the sandbox binary exists on this machine.

use mason_adapters::claude::AgentCommand;
use mason_adapters::sandbox::*;
use std::path::{Path, PathBuf};

fn policy(kind: SandboxKind) -> SandboxPolicy {
    SandboxPolicy {
        kind,
        worktree: "/work/wt".into(),
        state_dir: "/work/state".into(),
        extra: vec![],
    }
}

fn cmd() -> AgentCommand {
    AgentCommand {
        program: "codex".into(),
        args: vec!["exec".into(), "--json".into()],
        env: vec![("A".into(), "b".into())],
        cwd: "/work/wt".into(),
        stdin: "prompt".into(),
    }
}

#[test]
fn the_sandbox_exec_profile_allows_writes_only_where_named() {
    let p = sandbox_exec_profile(&policy(SandboxKind::SandboxExec)).unwrap();
    assert!(p.contains("(deny file-write*)"));
    assert!(p.contains("(subpath \"/work/wt\")") && p.contains("(subpath \"/work/state\")"));
    assert!(
        !p.contains("(deny network"),
        "the provider must stay reachable"
    );
    let w = wrap(cmd(), &policy(SandboxKind::SandboxExec)).unwrap();
    assert_eq!(w.program, PathBuf::from("sandbox-exec"));
    assert_eq!(&w.args[0], "-p");
    assert_eq!(&w.args[2..], ["codex", "exec", "--json"]);
    assert_eq!((w.env, w.stdin, w.cwd), (cmd().env, cmd().stdin, cmd().cwd));
}

#[test]
fn a_path_cannot_break_out_of_the_profile() {
    let mut p = policy(SandboxKind::SandboxExec);
    p.worktree = "/w\") (allow file-write* (subpath \"/".into();
    let s = sandbox_exec_profile(&p).unwrap();
    assert!(s.contains("\\\""), "{s}");
    p.worktree = "/w\n(allow default)".into();
    assert!(sandbox_exec_profile(&p).is_err());
}

#[test]
fn the_bubblewrap_argv_binds_the_machine_read_only_and_two_dirs_writable() {
    let w = wrap(cmd(), &policy(SandboxKind::Bubblewrap)).unwrap();
    assert_eq!(w.program, PathBuf::from("bwrap"));
    let a = w.args.join(" ");
    assert!(a.starts_with("--ro-bind / / --dev /dev --proc /proc --tmpfs /tmp"));
    assert!(a.contains("--bind /work/wt /work/wt") && a.contains("--bind /work/state /work/state"));
    assert!(a.contains("--die-with-parent") && a.contains("--chdir /work/wt"));
    assert!(!a.contains("--unshare-net"), "the network stays on");
    assert!(a.ends_with("-- codex exec --json"), "{a}");
}

#[test]
fn none_changes_nothing_and_parses_by_name() {
    assert_eq!(wrap(cmd(), &policy(SandboxKind::None)).unwrap(), cmd());
    assert_eq!(
        SandboxKind::parse("bubblewrap").unwrap(),
        SandboxKind::Bubblewrap
    );
    assert!(SandboxKind::parse("docker").is_err());
}

#[test]
fn a_missing_sandbox_binary_is_an_error_not_an_unsandboxed_run() {
    // Whichever of the two is absent here must refuse.
    for k in [SandboxKind::SandboxExec, SandboxKind::Bubblewrap] {
        if !available(k) {
            assert!(apply(cmd(), Some(&policy(k))).is_err());
        }
    }
    assert!(apply(cmd(), None).is_ok());
}

/// Run `sh` under a real sandbox: a write inside the worktree works, one outside fails.
fn live(kind: SandboxKind) {
    let wt = tempfile::tempdir_in("/tmp").unwrap();
    let st = tempfile::tempdir_in("/tmp").unwrap();
    // Outside every allowed path (and not scratch space): next to this crate.
    let outside = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
    let pol = SandboxPolicy::new(kind, wt.path(), st.path());
    let script = format!(
        "echo in > '{}/in.txt' && ! (echo out > '{}/out.txt') 2>/dev/null",
        pol.worktree.display(),
        outside.path().display()
    );
    let c = AgentCommand {
        program: "sh".into(),
        args: vec!["-c".into(), script],
        env: vec![],
        cwd: wt.path().into(),
        stdin: String::new(),
    };
    let w = apply(c, Some(&pol)).unwrap();
    let s = std::process::Command::new(&w.program)
        .args(&w.args)
        .current_dir(Path::new(&w.cwd))
        .status();
    assert!(s.is_ok_and(|s| s.success()), "the sandboxed script failed");
    assert!(wt.path().join("in.txt").exists());
    assert!(!outside.path().join("out.txt").exists());
}

#[test]
fn sandbox_exec_really_confines_writes_when_it_is_installed() {
    if !available(SandboxKind::SandboxExec) {
        eprintln!("sandbox-exec not installed: live run skipped");
        return;
    }
    eprintln!("sandbox-exec: live run");
    live(SandboxKind::SandboxExec);
}

#[test]
fn bubblewrap_really_confines_writes_when_it_is_installed() {
    if !available(SandboxKind::Bubblewrap) {
        eprintln!("bwrap not installed: live run skipped");
        return;
    }
    eprintln!("bwrap: live run");
    live(SandboxKind::Bubblewrap);
}

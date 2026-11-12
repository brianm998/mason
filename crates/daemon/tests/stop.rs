// SPDX-License-Identifier: GPL-3.0-or-later

//! A deliberate stop is logged and is not a crash, and a registration file
//! edit is applied without a restart.

mod support;
use mason_definition::tiers::{Level, Tier};
use support::*;

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

fn signal(pid: u32, name: &str) {
    let st = std::process::Command::new("kill")
        .args([&format!("-{name}"), &pid.to_string()])
        .status()
        .unwrap();
    assert!(st.success());
}

#[test]
fn sigterm_is_logged_recorded_as_stopped_and_exits_zero() {
    let s = Scratch::new("true", happy_script());
    let log = s.root.path().join("masond.log");
    let log_s = log.display().to_string();
    let mut m = s.masond(
        &["--runtime-sha", SHA],
        &[("MASON_TEST_STDERR", &log_s), ("RUST_LOG", "info")],
    );
    s.wait_daemon(&mut m);
    signal(m.child.id(), "TERM");
    let status = m.child.wait().unwrap();
    assert!(status.success(), "exit status {status}");
    let err = std::fs::read_to_string(&log).unwrap();
    assert!(err.contains("masond stopping: SIGTERM"), "{err}");
    let boots = std::fs::read_to_string(s.data_dir.join("runtimes/boots.log")).unwrap();
    let last = boots.lines().last().unwrap_or_default();
    assert!(
        last.ends_with(&format!("{SHA} stopped")),
        "boots.log: {boots}"
    );
}

#[test]
fn a_registration_edit_is_applied_without_a_restart() {
    let s = Scratch::new("true", happy_script());
    let d = s.start();
    let pid = d.project_id_by_name("scratch").unwrap();
    let model = |d: &mason_daemon::daemon::Daemon| {
        let p = d.project(pid).unwrap();
        let e = mason_daemon::tiermap::effective(
            &s.config_dir,
            mason_daemon::config::Provider::Claude,
            &p.overlay_text,
        );
        e.mapping
            .get(Tier::Light, Level::Low)
            .and_then(|(m, _)| m.model.clone())
    };
    assert_ne!(model(&d).as_deref(), Some("reloaded-model"));

    let file = s.config_dir.join("projects/scratch.toml");
    let text = std::fs::read_to_string(&file).unwrap();
    std::fs::write(
        &file,
        format!("{text}\n[provider.claude.light]\nlow = {{ model = \"reloaded-model\" }}\n"),
    )
    .unwrap();
    let changed = d.reload_registration("scratch").unwrap();
    assert_eq!(changed, vec!["provider".to_string()]);
    assert_eq!(model(&d).as_deref(), Some("reloaded-model"));
    // Nothing more to apply.
    assert!(d.reload_registration("scratch").unwrap().is_empty());

    // The base branch is fixed while masond runs: refused, naming it, and nothing changes.
    let edited = std::fs::read_to_string(&file)
        .unwrap()
        .replace("base = \"main\"", "base = \"other\"");
    std::fs::write(&file, edited).unwrap();
    let errs = d.reload_registration("scratch").unwrap_err();
    assert!(
        errs[0].contains("`base`") && errs[0].contains("Restart"),
        "{errs:?}"
    );
    assert_eq!(d.project(pid).unwrap().base, "main");
    assert_eq!(model(&d).as_deref(), Some("reloaded-model"));

    // A file that does not parse is refused too.
    std::fs::write(&file, "this is = = not toml").unwrap();
    assert!(d.reload_registration("scratch").is_err());
    // And a project masond never started with needs a restart.
    assert!(d.reload_registration("nope").unwrap_err()[0].contains("restart"));
}

#[test]
fn mason_project_reload_applies_the_edit_through_the_daemon() {
    let s = Scratch::new("true", happy_script());
    let mut m = s.masond(&[], &[]);
    s.wait_daemon(&mut m);
    let file = s.config_dir.join("projects/scratch.toml");
    let text = std::fs::read_to_string(&file).unwrap();
    std::fs::write(
        &file,
        text.replace("step_timeout_s = 120", "step_timeout_s = 121"),
    )
    .unwrap();
    let out = s.mason_ok(&["project", "reload"]);
    assert!(
        out.contains("Reloaded") && out.contains("step_timeout_s"),
        "{out}"
    );
    assert!(s
        .mason_ok(&["project", "reload"])
        .contains("Nothing changed"));
    std::fs::write(
        &file,
        std::fs::read_to_string(&file)
            .unwrap()
            .replace("main", "other"),
    )
    .unwrap();
    let o = s.mason(&["project", "reload"]);
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stdout).contains("`base`"));
}

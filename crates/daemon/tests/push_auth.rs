// SPDX-License-Identifier: GPL-3.0-or-later
#![cfg(unix)]
// Unix-only: signals, permission bits or unix sockets. Windows coverage comes with the later port tasks.

//! A push the host refuses for credentials holds landing for the project instead of failing the
//! task; the alert names the account the host saw; choosing a key whose probe gets through lifts
//! the hold and the held batch lands.

mod support;
use mason_core::*;
use mason_daemon::attention::{paused, What};
use std::path::Path;
use support::lw::*;
use support::*;

const REFUSAL: &str = "ERROR: Permission to brianm998/starry.git denied to brian-preffect.";

fn refuse_pushes(origin: &Path, flag: &Path) {
    let hook = origin.join("hooks/pre-receive");
    std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
    std::fs::write(
        &hook,
        format!(
            "#!/bin/sh\nif [ -e '{}' ]; then echo '{REFUSAL}' >&2; exit 1; fi\nexit 0\n",
            flag.display()
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn alert(w: &W, key: &str) -> Option<String> {
    mason_daemon::attention::sync_alerts(&w.d);
    w.d.store
        .alerts(true)
        .unwrap()
        .into_iter()
        .find(|a| a.key == key)
        .map(|a| a.text)
}

#[test]
fn a_refused_push_holds_landing_and_choosing_a_key_lets_the_batch_land() {
    let log = std::sync::Mutex::new(std::path::PathBuf::new());
    let w = world_with(
        "mode = \"direct-ci\"\nwindow = \"2s\"\nmax_size = 2\nmax_fold = 4\n[[tier]]\nname = \"batch\"\nsuites = [\"alpha\"]\n",
        &|s| {
            let (sec, l) = ci_section(s);
            *log.lock().unwrap() = l;
            sec
        },
        &[],
    );
    let flag = w.s.root.path().join("refuse");
    std::fs::write(&flag, "").unwrap();
    refuse_pushes(&w.s.origin, &flag);
    let t = w.task("one", &[("x/a.txt", "1")]);

    wait_for("landing to be held", 90, || {
        paused(&w.d, "scratch", What::Landing).is_some()
    });
    let p = paused(&w.d, "scratch", What::Landing).unwrap();
    assert_eq!(p.owner, "masond:push-auth");
    let text = alert(&w, "push-auth:scratch").expect("the alert is raised");
    assert!(text.contains("brian-preffect"), "{text}");
    assert!(!w.landed(t));
    assert_ne!(state_of(&w.d, t), State::Failed, "no failure is charged");
    assert!(
        paused(&w.d, "other", What::Landing).is_none(),
        "this project only"
    );
    std::thread::sleep(std::time::Duration::from_secs(3));
    assert_ne!(state_of(&w.d, t), State::Failed);

    // A key to choose.
    let ssh = w.s.root.path().join("dot-ssh");
    std::fs::create_dir(&ssh).unwrap();
    let key = ssh.join("id_test");
    assert!(std::process::Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-f"])
        .arg(&key)
        .status()
        .unwrap()
        .success());
    let pid = *w.d.projects().keys().next().unwrap();
    let repo = w.d.project(pid).unwrap().path.clone();

    // A probe that fails keeps the hold and gives back git's words.
    let err = mason_daemon::ssh_keys::choose_in(
        &w.d,
        "scratch",
        &repo,
        "/nonexistent/remote.git",
        &ssh,
        key.to_str().unwrap(),
        None,
    )
    .unwrap_err();
    assert!(err.contains("landing stays held"), "{err}");
    assert!(paused(&w.d, "scratch", What::Landing).is_some());
    let cfg = mason_landing::git::run(&repo, &["config", "--local", "core.sshCommand"]).unwrap();
    assert!(
        cfg.contains("id_test") && cfg.contains("IdentitiesOnly=yes"),
        "{cfg}"
    );

    // One that gets through lifts it; the owner's fix lets pushes through too.
    std::fs::remove_file(&flag).unwrap();
    let ok = mason_daemon::ssh_keys::choose_in(
        &w.d,
        "scratch",
        &repo,
        w.s.origin.to_str().unwrap(),
        &ssh,
        key.to_str().unwrap(),
        None,
    )
    .unwrap();
    assert_eq!(ok["ok"], true);
    assert!(paused(&w.d, "scratch", What::Landing).is_none());
    assert!(
        alert(&w, "push-auth:scratch").is_none(),
        "the alert is gone"
    );
    wait_for("the held batch to land", 90, || w.landed(t));
    assert!(w.on_main("x/a.txt"));
}

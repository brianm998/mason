// SPDX-License-Identifier: GPL-3.0-or-later
#![cfg(unix)]
// Unix-only: signals, permission bits or unix sockets. Windows coverage comes with the later port tasks.

//! A repository's `core.sshCommand` is honoured by the network git MASON runs, with MASON's
//! never-ask options appended. One test, because it edits the process environment.

use mason_landing::git;
use std::os::unix::fs::PermissionsExt;

#[test]
fn core_ssh_command_is_the_base_and_batch_mode_is_appended() {
    std::env::remove_var("GIT_SSH_COMMAND");
    std::env::remove_var("GIT_SSH");
    std::env::remove_var("GIT_ALLOW_PROTOCOL");
    let d = tempfile::tempdir().unwrap();
    let log = d.path().join("argv.txt");
    let fake = d.path().join("fake-ssh.sh");
    std::fs::write(
        &fake,
        format!("#!/bin/sh\necho \"$@\" > {}\nexit 1\n", log.display()),
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let repo = d.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    git::run(&repo, &["init", "-q"]).unwrap();
    git::run(
        &repo,
        &[
            "config",
            "core.sshCommand",
            &format!("{} -i /nonexistent/key", fake.display()),
        ],
    )
    .unwrap();
    let r = git::run(
        &repo,
        &["ls-remote", "ssh://git@example.invalid/x.git", "HEAD"],
    );
    let _ = r;
    let argv = std::fs::read_to_string(&log).expect("the fake ssh was run");
    assert!(argv.contains("-i /nonexistent/key"), "{argv}");
    assert!(
        argv.contains(
            "-o BatchMode=yes -o ConnectTimeout=30 -o ServerAliveInterval=15 -o ServerAliveCountMax=4"
        ),
        "{argv}"
    );
}

// SPDX-License-Identifier: GPL-3.0-or-later

//! Stopping the command of a `mason run` whose task's step has ended (s688).
//!
//! The waiting requests of an ended step are simply closed by the broker. A granted one has a
//! command running that nobody will read: the command (the `mason run` client's children) gets
//! SIGTERM to its process group, SIGKILL after a grace period, and its units are released.

use crate::hung::{read_ps, tree_of, Proc};
use mason_platform::process::{signal_group, signal_process, Signal};
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

/// How long a command has to end on SIGTERM before it is killed.
pub fn grace() -> Duration {
    std::env::var("MASON_RUN_STOP_GRACE_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .map_or(Duration::from_secs(10), Duration::from_millis)
}

/// What to signal to stop the command `client` (a `mason run`) is running: the process groups
/// the command leads, and its single processes where it shares the client's own group. The
/// client itself is left to notice its child has gone and release the lease.
pub fn targets(procs: &[Proc], client: u32) -> (BTreeSet<u32>, BTreeSet<u32>) {
    let home = procs.iter().find(|p| p.pid == client).map(|p| p.pgid);
    let own = mason_platform::process::own_group();
    let (mut groups, mut singles) = (BTreeSet::new(), BTreeSet::new());
    for p in tree_of(procs, client) {
        if p.pid == client {
            continue;
        }
        if p.pgid > 1 && p.pgid != own && Some(p.pgid) != home {
            groups.insert(p.pgid);
        } else if p.pid > 1 {
            singles.insert(p.pid);
        }
    }
    (groups, singles)
}

fn signal(procs: &[Proc], client: u32, sig: Signal) {
    let (groups, singles) = targets(procs, client);
    for g in groups {
        let _ = signal_group(g, sig);
    }
    for p in singles {
        let _ = signal_process(p, sig);
    }
}

/// SIGTERM the command of `client`, wait up to `grace` for `done()`, then SIGKILL it. Blocks.
/// A pid of 0 (the requester gave none) has nothing to stop.
pub fn stop_command(client: u32, grace: Duration, done: impl Fn() -> bool) {
    if client == 0 {
        return;
    }
    signal(&read_ps(), client, Signal::Term);
    let until = Instant::now() + grace;
    while Instant::now() < until && !done() {
        std::thread::sleep(Duration::from_millis(50));
    }
    if !done() {
        signal(&read_ps(), client, Signal::Kill);
        // The client may be between its grant and starting the command.
        let _ = signal_process(client, Signal::Kill);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: u32, ppid: u32, pgid: u32) -> Proc {
        Proc {
            pid,
            ppid,
            pgid,
            cpu_ms: 0,
            stopped: false,
        }
    }

    #[test]
    fn the_commands_groups_are_signaled_and_the_client_is_not() {
        // client 10 (group 5) runs 11 (own group) with a grandchild 12 in 11's group,
        // and 13, which shares the client's group.
        let procs = [
            p(10, 1, 5),
            p(11, 10, 11),
            p(12, 11, 11),
            p(13, 10, 5),
            p(99, 1, 99),
        ];
        let (g, s) = targets(&procs, 10);
        assert_eq!(g, BTreeSet::from([11]));
        assert_eq!(s, BTreeSet::from([13]));
    }

    #[test]
    fn a_command_is_stopped_with_its_group_and_the_client_is_left() {
        // `sh` stands for the `mason run` client; its `sleep` is the command, in its own group.
        let mut client = std::process::Command::new("sh")
            .args(["-c", "sleep 300 & wait"])
            .spawn()
            .unwrap();
        let pid = client.id();
        // The command exists once `ps` shows a child of the client.
        let has_child = || read_ps().iter().any(|p| p.ppid == pid);
        while !has_child() {
            std::thread::sleep(Duration::from_millis(20));
        }
        let still = std::cell::Cell::new(true);
        stop_command(pid, Duration::from_secs(5), || {
            let gone = !read_ps().iter().any(|p| p.ppid == pid);
            still.set(!gone);
            gone
        });
        assert!(!still.get());
        let _ = client.kill();
        let _ = client.wait();
    }
}

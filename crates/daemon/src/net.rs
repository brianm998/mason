// SPDX-License-Identifier: GPL-3.0-or-later

//! Transient network failures (#1185, decision 1133).
//!
//! A few minutes without DNS must not fail a landing batch, use up a task's test rounds or be
//! blamed on the change. This module says which errors are the network's, retries the git calls
//! that meet one, and keeps the host-wide record the one "network outage" alert is made from.
//! A refused credential is never the network's: waiting does not change it.

use std::sync::Mutex;
use std::time::Duration;

/// Name resolution failed, or there is no way to the host. Only the machine's network (or the
/// far side's) says these.
const UNREACHABLE: [&str; 10] = [
    "could not resolve host",
    "couldn't resolve host",
    "nodename nor servname provided",
    "temporary failure in name resolution",
    "name or service not known",
    "network is unreachable",
    "network is down",
    "no route to host",
    "failed to connect to",
    "couldn't connect to server",
];

/// A connection git or curl lost part way. Worded by git and curl, so a log saying one of them
/// is about a fetch or a push.
const CUT_OFF: [&str; 3] = ["rpc failed", "early eof", "the remote end hung up"];

/// Said by git and ssh, and also by many programs and tests about connections of their own:
/// matched in the error of a git call, never in a suite's log.
const GIT_ONLY: [&str; 2] = ["connection reset", "timed out"];

/// Lines at the end of a suite's log that are looked at.
const LOG_TAIL: usize = 40;

/// Whether an error is the credential (or the account's rights) being refused.
pub fn refused(e: &str) -> bool {
    let l = e.to_ascii_lowercase();
    [
        "authentication failed",
        "permission denied",
        "could not read username",
        "invalid username or password",
        "bad credentials",
        "error: 401",
        "error: 403",
        "protected branch",
    ]
    .iter()
    .any(|m| l.contains(m))
}

fn server_error(l: &str) -> bool {
    ["500", "502", "503", "504"]
        .iter()
        .any(|c| l.contains(&format!("error: {c}")))
}

/// Whether the error of a git call is the network's: name resolution, no route, a lost or timed
/// out connection, a 5xx. A rejection the remote decided on is not.
pub fn is_network(e: &str) -> bool {
    if refused(e) {
        return false;
    }
    let l = e.to_ascii_lowercase();
    UNREACHABLE
        .iter()
        .chain(&CUT_OFF)
        .chain(&GIT_ONLY)
        .any(|m| l.contains(m))
        || server_error(&l)
}

/// [`is_network`], or the remote rejecting a push for a reason of its own ("remote rejected"):
/// the next try may well work (#772).
pub fn is_transient(e: &str) -> bool {
    !refused(e) && (is_network(e) || e.to_ascii_lowercase().contains("remote rejected"))
}

/// The line at the end of a suite's log that says the suite could not reach the network, if one
/// does. Narrower than [`is_network`]: "timed out" and "connection reset" are things a test
/// says about the product.
pub fn log_failure(output: &str) -> Option<String> {
    let lines: Vec<&str> = output.lines().collect();
    let tail = &lines[lines.len().saturating_sub(LOG_TAIL)..];
    if tail.iter().any(|l| refused(l)) {
        return None;
    }
    tail.iter().rev().find_map(|line| {
        let l = line.to_ascii_lowercase();
        (UNREACHABLE.iter().chain(&CUT_OFF).any(|m| l.contains(m)) || server_error(&l))
            .then(|| line.trim().chars().take(200).collect())
    })
}

/// Run `op`; while its error is the network's, `failed` is told, and after `sleep` of the next
/// wait of `backoff` it runs again. The first other result, or the error after the last wait.
pub fn retry<T, E: std::fmt::Display>(
    backoff: &[Duration],
    sleep: impl Fn(Duration),
    mut failed: impl FnMut(&str),
    mut op: impl FnMut() -> Result<T, E>,
) -> Result<T, E> {
    let mut waits = backoff.iter();
    loop {
        match op() {
            Err(e) => {
                let text = e.to_string();
                if !is_network(&text) {
                    return Err(e);
                }
                failed(&text);
                let Some(wait) = waits.next() else {
                    return Err(e);
                };
                tracing::warn!(
                    "the network failed, trying again in {} s: {text}",
                    wait.as_secs()
                );
                sleep(*wait);
            }
            ok => return ok,
        }
    }
}

/// The reason of every failure that is not about the code because the network was not there.
pub const SUITE_REASON: &str = "could not reach the network";

/// The network failures met lately, host-wide: `(when in ms, task or 0)`. In memory: an outage
/// is over long before a restart's reader would care.
#[derive(Default)]
pub struct Outages(Mutex<Vec<(u64, u64)>>);

/// Kept at most this long, whatever the alert's window is.
const KEEP_MS: u64 = 3_600_000;

impl Outages {
    pub fn note(&self, at_ms: u64, task: u64) {
        let mut v = self.0.lock().unwrap_or_else(|e| e.into_inner());
        v.retain(|(t, _)| t.saturating_add(KEEP_MS) >= at_ms);
        v.push((at_ms, task));
    }

    pub fn all(&self) -> Vec<(u64, u64)> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_resolution_and_no_network_errors_and_the_old_ones_are_the_networks() {
        for e in [
            "ssh: Could not resolve hostname github.com: nodename nor servname provided, or not known\nfatal: Could not read from remote repository.",
            "fatal: unable to access 'https://code.videolan.org/x264.git/': Could not resolve host: code.videolan.org",
            "getaddrinfo: nodename nor servname provided, or not known",
            "curl: (6) Temporary failure in name resolution",
            "ssh: connect to host github.com port 22: Network is unreachable",
            "ssh: Could not resolve hostname x: Name or service not known",
            "ssh: connect to host github.com port 22: No route to host",
            "fatal: Connection reset by peer",
            "error: RPC failed; curl 56",
            "fatal: early EOF",
            "fatal: the remote end hung up unexpectedly",
            "git push timed out: no answer within 300s",
            "The requested URL returned error: 503",
        ] {
            assert!(is_network(e), "{e}");
            assert!(is_transient(e), "{e}");
        }
        assert!(is_transient(" ! [remote rejected] x -> y (failed)"));
        assert!(!is_network(" ! [remote rejected] x -> y (failed)"));
    }

    #[test]
    fn a_refused_credential_or_a_rejected_push_is_not() {
        for e in [
            "git@github.com: Permission denied (publickey).\nfatal: Could not read from remote repository.",
            "remote: Invalid username or password\nfatal: Authentication failed",
            " ! [rejected] main -> main (non-fast-forward)\nerror: failed to push some refs",
            "fatal: Could not read from remote repository.",
            "fatal: not a git repository",
            // Refused wins over anything else in the same text.
            "Permission denied (publickey). Connection reset by peer",
        ] {
            assert!(!is_network(e), "{e}");
            assert!(!is_transient(e), "{e}");
        }
    }

    #[test]
    fn a_suite_log_is_read_at_its_end_and_not_for_a_tests_own_timeouts() {
        let log = "cloning x264\nfatal: Could not resolve host: code.videolan.org\nmake: *** [x264] Error 128\n";
        assert_eq!(
            log_failure(log).as_deref(),
            Some("fatal: Could not resolve host: code.videolan.org")
        );
        assert_eq!(
            log_failure("assertion failed: the request timed out\n"),
            None
        );
        assert_eq!(log_failure("Connection reset by peer\n"), None);
        let early = format!("Could not resolve host: x\n{}", "more\n".repeat(LOG_TAIL));
        assert_eq!(log_failure(&early), None);
        assert_eq!(
            log_failure("Could not resolve host: x\nPermission denied (publickey).\n"),
            None
        );
    }

    #[test]
    fn a_network_error_is_retried_after_each_wait_and_another_error_is_not() {
        let waits = [1, 3, 9].map(Duration::from_secs);
        let slept = std::cell::RefCell::new(vec![]);
        let run = |fails: usize, why: &str| {
            let (mut calls, mut told) = (0, 0);
            let r: Result<(), String> = retry(
                &waits,
                |d| slept.borrow_mut().push(d.as_secs()),
                |_| told += 1,
                || {
                    calls += 1;
                    if calls <= fails {
                        Err(why.to_string())
                    } else {
                        Ok(())
                    }
                },
            );
            (r.is_ok(), calls, told, slept.take())
        };
        let dns = "Could not resolve hostname github.com";
        assert_eq!(run(2, dns), (true, 3, 2, vec![1, 3]));
        assert_eq!(run(9, dns), (false, 4, 4, vec![1, 3, 9]));
        assert_eq!(run(9, "non-fast-forward"), (false, 1, 0, vec![]));
    }
}

// SPDX-License-Identifier: GPL-3.0-or-later

//! The daemon's limit on open files. launchd starts a service with a soft limit
//! of 256, and everything the daemon runs inherits it. A cold `cargo build` on a
//! machine with many cores opens that many at once as its first wave of `rustc`
//! starts, and fails with "Too many open files". A shell's usual limit is far
//! higher, so the same build works when run by hand. Raising the soft limit
//! needs no privilege, as long as it stays within the hard limit.

#[cfg(unix)]
use nix::sys::resource::{getrlimit, setrlimit, Resource};

/// The most macOS accepts for the soft limit, even when the hard limit is
/// "unlimited" (`OPEN_MAX`), and what a macOS shell usually has.
#[cfg(any(unix, test))]
const WANTED: u64 = 10240;

/// Pure: the soft limit to move to, if that would be an increase.
#[cfg(any(unix, test))]
fn raised_soft(soft: u64, hard: u64) -> Option<u64> {
    let want = WANTED.min(hard);
    (want > soft).then_some(want)
}

/// Windows has no per-process open-file limit to raise (handles are bounded by
/// memory, not a soft limit), so there is nothing to do.
#[cfg(windows)]
pub fn raise() {}

/// Raise this process's soft limit, for itself and all it starts. Called first
/// thing at startup. A failure is logged and the daemon carries on as it was.
#[cfg(unix)]
pub fn raise() {
    let (soft, hard) = match getrlimit(Resource::RLIMIT_NOFILE) {
        Ok(limits) => limits,
        Err(e) => {
            tracing::warn!("could not read the open-file limit: {e}");
            return;
        }
    };
    let Some(new) = raised_soft(soft, hard) else {
        return;
    };
    match setrlimit(Resource::RLIMIT_NOFILE, new, hard) {
        Ok(()) => tracing::info!("raised the open-file limit from {soft} to {new}"),
        Err(e) => tracing::warn!("could not raise the open-file limit from {soft}: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_soft_limit_only_ever_goes_up_and_stays_within_the_hard_limit() {
        let unlimited = u64::MAX;
        assert_eq!(
            raised_soft(256, unlimited),
            Some(WANTED),
            "launchd's limits"
        );
        assert_eq!(
            raised_soft(256, 4096),
            Some(4096),
            "capped by the hard limit"
        );
        assert_eq!(raised_soft(WANTED, unlimited), None, "already there");
        assert_eq!(raised_soft(65536, unlimited), None, "never lowered");
        assert_eq!(raised_soft(1024, 1024), None, "no room to raise it");
    }
}

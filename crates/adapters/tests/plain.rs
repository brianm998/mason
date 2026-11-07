// SPDX-License-Identifier: GPL-3.0-or-later

//! Plain git passes the host kit for hosts with no features.

use mason_adapter_api::kit::plain_host_kit;
use mason_adapter_api::service::*;
use mason_adapters::plain::PlainGit;

#[test]
fn plain_git_passes_the_plain_host_kit() {
    let p = plain_host_kit(&PlainGit::new("ssh://git@example.invalid/srv/r.git"));
    assert!(p.is_empty(), "{p:?}");
}

#[test]
fn the_plain_kit_catches_a_host_that_pretends() {
    struct Liar;
    impl GitHostAdapter for Liar {
        fn name(&self) -> &'static str {
            "liar"
        }
        fn capabilities(&self) -> HostCaps {
            HostCaps {
                pull_requests: true,
                ..HostCaps::default()
            }
        }
        fn open_pr(&self, _: &str, _: &str, _: &str, _: &str) -> Result<PrRef, ApiError> {
            Ok(PrRef {
                number: 1,
                url: String::new(),
            })
        }
        fn pr_status(&self, _: u64) -> Result<PrStatus, ApiError> {
            Err(ApiError::new(ErrorKind::Unavailable, "x"))
        }
        fn merge_pr(&self, _: u64) -> Result<(), ApiError> {
            Ok(())
        }
        fn enqueue(&self, _: u64) -> Result<(), ApiError> {
            Ok(())
        }
        fn close_pr(&self, _: u64) -> Result<(), ApiError> {
            Ok(())
        }
    }
    assert!(plain_host_kit(&Liar).len() >= 4);
}

#[test]
fn an_unreachable_remote_is_reported_unavailable() {
    let e = PlainGit::new("/definitely/not/a/repo.git")
        .reachable()
        .unwrap_err();
    assert_eq!(e.kind, ErrorKind::Unavailable);
}

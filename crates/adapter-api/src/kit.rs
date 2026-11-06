// SPDX-License-Identifier: GPL-3.0-or-later

//! Conformance kits (Part 11): what any implementation of a service trait must
//! do, built in or external, run against a recorded or fake service. Each kit
//! returns the problems it found; an empty list is a pass.

use crate::service::*;

/// What a kit needs the service to hold.
pub struct HostScenario {
    pub base: String,
    /// A branch on the host, ahead of `base`, whose checks pass and that is
    /// mergeable (and, where reviews are required, approved).
    pub good_head: String,
    pub missing_head: String,
}

pub fn host_kit(h: &dyn GitHostAdapter, s: &HostScenario) -> Vec<String> {
    let mut p = vec![];
    let caps = h.capabilities();
    if !caps.pull_requests {
        return vec!["a git host that cannot open pull requests has nothing for this kit to check".into()];
    }
    match h.pr_status(999_999) {
        Err(e) if e.kind == ErrorKind::NotFound => {}
        other => p.push(format!("the status of a PR that does not exist must be NotFound, got {other:?}")),
    }
    match h.open_pr(&s.base, &s.missing_head, "t", "b") {
        Err(e) if e.permanent() => {}
        other => p.push(format!("a PR from a branch that is not there must be refused for good, got {other:?}")),
    }
    let pr = match h.open_pr(&s.base, &s.good_head, "A change", "Why.") {
        Ok(pr) => pr,
        Err(e) => return vec![format!("opening a PR from a good branch failed: {e}")],
    };
    if pr.number == 0 {
        p.push("a PR has a number".into());
    }
    match h.pr_status(pr.number) {
        Ok(st) => {
            if st.state != PrState::Open {
                p.push(format!("a new PR is open, not {:?}", st.state));
            }
            if st.head.is_empty() {
                p.push("a status names the head commit".into());
            }
        }
        Err(e) => p.push(format!("the status of a new PR: {e}")),
    }
    if caps.merge_queue {
        // (Not exercised here: it would consume the PR.)
    } else if h.enqueue(pr.number).is_ok() {
        p.push("a host that declares no merge queue must refuse to enqueue".into());
    }
    if let Err(e) = h.merge_pr(pr.number) {
        p.push(format!("a green, reviewed PR must merge: {e}"));
    }
    match h.pr_status(pr.number) {
        Ok(st) if st.state == PrState::Merged => {}
        other => p.push(format!("after a merge the PR is Merged, got {other:?}")),
    }
    if h.merge_pr(pr.number).is_ok() {
        p.push("merging a merged PR twice must be refused".into());
    }
    p
}

pub fn tracker_kit(t: &dyn TrackerAdapter) -> Vec<String> {
    let mut p = vec![];
    let caps = t.capabilities();
    let (items, etag) = match t.list(None) {
        Ok(Listing::Items { items, etag }) => (items, etag),
        other => return vec![format!("an unconditional list returns items, got {other:?}")],
    };
    if items.iter().any(|i| i.number == 0 || i.title.is_empty()) {
        p.push("every item has a number and a title".into());
    }
    if items.iter().any(|i| !["urgent", "high", "normal", "low"].contains(&i.priority.as_str())) {
        p.push("an item's priority is one of urgent, high, normal, low".into());
    }
    if caps.conditional_polling {
        match (&etag, t.list(etag.as_deref())) {
            (Some(_), Ok(Listing::NotModified)) => {}
            (None, _) => p.push("a tracker that polls conditionally returns an ETag".into()),
            (_, other) => p.push(format!("asking again with the ETag must say NotModified, got {other:?}")),
        }
    }
    p
}

pub fn build_kit(b: &dyn BuildSystemAdapter, git_ref: &str, sha: &str) -> Vec<String> {
    let mut p = vec![];
    if !b.capabilities().arbitrary_ref {
        p.push("a build system must run on an arbitrary ref to be used for candidate refs".into());
    }
    match b.trigger(git_ref, sha) {
        Ok(run) => match b.status(&run) {
            Ok(_) => {}
            Err(e) => p.push(format!("the status of a run just started: {e}")),
        },
        Err(e) => p.push(format!("triggering a run: {e}")),
    }
    p
}

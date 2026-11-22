# 0798: CI-watch tickets for confirmed reds can be filed approved

Status: proposed, waits for the owner (changes who approves work; `crates/daemon/src/ci_watch.rs`).

## Context

The base-branch CI watch files every ticket as proposed. From 2026-11-13 to 11-20 the owner approved 69 of them, a median of 1.8 h after filing (p90 7.1 h), while develop stayed red.

## Decision

- `[watch.ci] auto_approve = "confirmed"` (default `"off"`) files a ticket approved when its red was seen on two consecutive finished runs that ran the job and no open task covers the same job and test.
- A ticket that names a trust file (`protect::PROTECTED`, in a failing script or test or a path in the quoted log) stays proposed.
- `auto_approve_max` (default 5) caps auto-approved tickets open at once; beyond it they stay proposed.
- Shards of one job with the same failing script fold into one ticket, and one failure on two systems is one ticket naming both.
- `mason self` / the CI-watch view mark auto-approved tickets so the owner can cancel them.

## Consequences

A wrong red can start work without the owner; the cap and the cancel marker bound that. Not enabled for MASON: that edit is in `.mason/**` and is the owner's.

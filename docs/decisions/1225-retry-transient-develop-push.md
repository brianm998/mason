<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# 1225: retry a develop push the remote rejects transiently

Task #1247. Stage s1235.

## Decision

`promote()` retries a failed push when the error says `remote rejected`, is not a refusal (auth, `protected branch`), and the base did not move. Waits are the caller's `push_backoff` (the daemon's `network_backoff`: 10 s, 30 s, 90 s). Before each new try it fetches: a moved base is `Moved`; a base that already holds the commit is success. Network errors are left to `over_network` (#1185), so the two do not stack.

The rule for a refusal lives once, in the landing crate; the daemon's `net::refused` calls it.

## What holds the lock during the waits

The batch's repository lock is held across `promote()`, so it is held during the waits (up to 130 s). Releasing it would let another landing push first, which the next fetch would report as `Moved` and force a fold and re-verification, worse than waiting. Direct landing takes no lock here.

## Not verified

Behaviour against GitHub's real rejections; only a hook-rejecting bare remote was tried.

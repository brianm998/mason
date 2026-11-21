# 0784: The landing lint tier runs on this Mac

Status: owner approved in chat on 2026-11-21 (task #806). It touches `crates/daemon/src/landing.rs` and the `[[tier]]` keys of `landing.toml` (verdict/trust files). The lint commands, the output parser and the gate are unchanged; only where the lint runs changes.

Decision: `[[tier]]` takes `where = "local" | "github"`. The lint tier defaults to `local`: each lint suite's command runs in the batch's worktree under the local `compile` slot it already took, whatever the global switch says. With `where = "github"` the check goes through the one placement function with the suite and the batch's first task (so the suite's `remote` and `runs_on` and the switch decide), and a check that goes to GitHub takes no local slot while it waits (as #759/#761 did for run_check). The batch note says `lint: clean (on this Mac, 1m12s)` or `(on GitHub Actions, ...)`.

Why: lint is 1.5-2 min of work behind up to 20 min of runner queue (batch #496 failed with no runner in 1200 s), and it held a compile slot while waiting.

Other landing checks that use `global: true` (the batch re-test and build in the rebase loop, the fast tier, the re-test around line 3095 and 3425 of landing.rs, and `jobs.rs` 2551) are unchanged: test tiers and full-suite runs stay where the switch puts them.

Not verified: the tests and exit script run only on GitHub/in the test step; I could not run them locally. The `where = "github"` test uses a silent TCP listener, not the real fake-github binary.

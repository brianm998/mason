# 0425: the pipeline checks itself, and a health session proposes or acts

Decision: the daemon gathers facts on an interval, a pure function in core judges them, and a finding not handled within a cooldown starts one owner session of kind `health` that ends in `submit_health`. Every possible solution is filed as a proposal; the session acts only through actions the owner already has; only when the daemon itself finds that nothing can land does the session get a writable branch.

## Why a session and not more watchers

The watchers each do one thing and none look across tasks. A model session can read logs, the store and `mason` read commands and say what is wrong. It does not decide for the owner: its default output is a proposal, and the filing rules (dedupe, evidence, caps) are the existing ones.

## Choices

- **Pure judgement in core**, facts in the daemon, so the rule can be tested with a fake clock and never sleeps. Keys are stable (`stuck:task:7`, `overloaded:slots`) so the cooldown can say "already handled".
- **At most one session, never while the account is held**, on `light` unless a finding is severe. Cost counts like any session: it is an owner session.
- **`HEALTH_ACTIONS` in the contract.** `retry`, `unblock`, `release_holder` and `rerun_diagnosis` are each something the owner can already do, and each is recorded as an intervention by `health`. `unblock` calls the daemon's owner-only path under the first configured owner's name; the intervention row names `health`. A diagnosis's rules (`diagnose.rs`) are untouched: the session can only re-run one.
- **Direct fix.** The condition is computed by the daemon (`cannot_land`), not taken from the session. The worktree is `claude/health-<id>` from the base. A protected path is named and waits for the owner as always. Nothing is pushed or landed by this code: the branch goes to the owner. That is deliberate, see below.
- **Proposals dedupe** by the finding key and title in `health_log`, on top of `proposals::file`'s own text rules.
- **`GET /health` and `mason health` are already the detectors' report.** The pipeline report is `mason health pipeline`, and the old report gains a `pipeline` key, so scripts that read the old one still work.

## Trust files (for the owner's approval)

- `crates/adapter-api/src/contract.rs`: the `submit_health` tool and `HEALTH_ACTIONS`. It adds a tool for a new session kind; it removes no check. An action outside the list is refused by validation. The tool does not widen what any other step may call.
- The direct-fix authority. It is narrower than the ticket describes: it gives a session a writable branch under a daemon-computed condition and goes no further than a branch. No gate, budget or verdict is changed, and there is no new way to `develop`.
- `clients/features.toml`: two features, done in both clients.
- `scripts/exit/s432.sh`: a new exit script.

## Rejected

- Landing the fix from the daemon. A path that lands "ahead of the queue" is a new route to `develop`. It should be its own reviewed change, with the checks run by the pipeline, not by this code.
- Starting a session for every finding. The cooldown, the single session and the account hold bound the cost.

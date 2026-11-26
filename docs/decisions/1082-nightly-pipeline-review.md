# 1082: the weekly analyst becomes the nightly pipeline review

## Context

The owner ran a review of pipeline health by hand about once a week. The analyst did something close, but asked a generic question per project, ran at the project's tier on a fixed seven-day timer, required a metric, and compared filings only with recent proposals. The owner asked on 2026-11-24 for it to be their review: default on, nightly at midnight, Opus at medium, filings proposed only.

## Decisions

- **One review for the pipeline**, a Low task in MASON's own project (`[self]`), covering every project with `[introspect] enabled` and `analyst` on. With no `[self]` project there is no review, and `mason review status` says so.
- **Settings in `<config>/review.toml`**, read on every tick, written through a temporary file and a rename, with `changed_by` and `changed_at`. A missing file is the defaults. A file that cannot be read turns the review off and reports why, so a typo never runs it at the wrong time.
- **Due** by the time of day (`ci_schedule::due_at`) and by `every_days` local calendar days since the last *completed* run started. Counting calendar days, not 24-hour spans, keeps a nightly run nightly when the previous one started a minute late or was a daytime catch-up. A run that was filed but never completed is tried again at the next start time, not on the next tick. The offset is the present one, so a start time may be off by the daylight-saving step on the day it changes.
- **Filed as proposed, then approved by the daemon.** The task's model and effort overrides and the schedule row that makes it the review are written while it is still proposed, so nothing can claim it before they exist. A daemon that stops in between approves it on the next tick.
- **The effort is pinned at launch.** As a task override `medium` is an abstract level, which for Claude's deep tier maps to no effort at all. `layers.rs` (protected) is unchanged: `review::pin_effort` gives the session the effort the task carries.
- **A review task is not an ordinary change.** It takes no stage or decision number (one a night would use them up). Its submission is checked for the tip and a complete result file instead of for commits and a clean tree; its branch is never landed.
- **Duplicates**: `Filing::lookback = Some(All)` compares with every task of the target project in every state, with no time limit. Other callers pass `None` and behave as before.
- **Limits**: review filings are not held to `[proposals] per_task` or `per_day` (the review's `max_proposals` caps them), and in MASON's own project they are checked with `selfguard::check_filing_proposal` instead of `check_filing_as`, so `max_open`, `per_week` and `max_steps` do not refuse them. A proposed filing spends nothing (decision 0046); the rollback cooldown still refuses. Without this a busy pipeline would refuse most of a night's findings. **This is a change to which limits apply, for the owner to approve.**
- **Categories**: a review finding names `stuck`, `speed`, `narrow`, `coverage`, `crash` or `automate`; a metric is optional. The detectors still need a metric. Findings are filed under the rule `review`.
- **The notice** is an alert of kind `review` that lasts a day, routed to the monitor and speech, and never reminded of.
- `analyst_interval`, `--analyst-ms` and the `analyst:<project>` rows are gone. The rows are deleted at the first tick.

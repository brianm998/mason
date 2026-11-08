# 0046: Tracker imports start proposed, the readings and the trust holds

Task #39 (stage s65). Each is the reading taken; change one and the others stand.

## Trust: this waits for the owner

- **What changes.** An issue the tracker imports no longer starts work by itself. By default it becomes a *proposed* task and waits for `mason task approve` (or the monitors' Approve). This **narrows** what an outside signal may start.
- **`import_as = "queued"` restores the old behaviour**, and it is read only from the project file in the owner's machine config (`[tracker] import_as`). Nothing in an issue (title, body, label, author) can set it; an unknown value is refused when the config loads, naming the key.
- **Files this touches on the trust path:** `crates/daemon/src/tracker.rs`, `config.rs`, `http.rs` (what an outside signal may start, and a new owner-only `relink`), and `crates/daemon/src/selfguard.rs`, a protected guard file, which **loosens** a guard: proposed filings are no longer held to `max_open`, `per_week` or `max_steps`. `clients/features.toml` is also protected. Both **wait for the owner's approval**; this record is why.
- **Why loosening is safe.** A proposed task spends nothing and starts nothing; the owner's approval is the gate. It is the same reasoning as decisions 0022 and 0032 (held and proposed tasks are not counted). The cooldown after a rollback still refuses a proposed filing.

## Readings

- **Proposed filings and the limits.** New `selfguard::check_filing_proposal` checks the cooldown only. The tracker (in `proposed` mode), `POST /tasks` with `proposed: true` and so `mason task add --proposed` use it. `mason task add` without `--proposed` is checked as before. `count()` and `open_counts()` are unchanged: a proposed task is already idle there, so it is not in `open` or `held`. It is still filed, so it counts in the week's `per_week` figure when it is not the owner's; that figure only matters to queued filings. Approving a proposed task is not checked against a limit either: the owner decides.
- **Decision 0022's owner exemption** is unaffected and covers imports by an owner name: in `queued` mode they are still exempt from `per_week` and `max_steps` and held to `max_open`; in `proposed` mode they are held to nothing.
- **A refusal is visible.** A queued import a limit (or the cooldown) refuses is recorded per pass; the alert `tracker-pending` reads `N issues not imported (#a, #b): <reason>`, grouped by reason, and clears when the pass has nothing left. `GET /pending-issues` and `mason task list --pending-issues` list them with the reason; both monitors show a "not imported" tile and a list, only while there are some. The list is rewritten on every pass and lives in memory (it is derived, like the other tracker alerts).
- **Adopt, don't duplicate.** An open, eligible issue with no tracker row is linked to a live task of the same project whose title ends in `(GitHub #N)` and that no row already points to. "Live" is not landed, cancelled or failed. The row takes the issue's current title, body and priority; the task keeps its own title (so no edit is sent to it). A cancelled or landed task is never adopted, so the issue is imported afresh.
- **A cancelled task behind an open issue.** The issue stays tied to the cancelled task (the row is kept), and the alert says so and names `mason task relink <task> --issue N`. `relink` (CLI, `POST /tasks/{id}/relink`) is for the owner's identity only (`x-mason-as`), and refuses a task that is landed or cancelled, a task already linked to another issue, an issue whose task is live and not cancelled, an issue that is not a tracker item and a closed issue. Failed tasks may be linked (they can be retried). It is recorded as an intervention.
- **Closure and edits** are unchanged: a closed issue cancels its task (proposed included); edits reach a proposed task like a queued one.
- **A proposed task with blockers** shows `Proposed, held behind #N` in `why_waiting`; it is not counted held, since held is a queued state. Dependencies still import in order.
- **Monitors.** Proposed tasks that came from the tracker are in their own sidebar section `proposed_imports` ("Proposed from the tracker"), after Proposed and present only while there are some, with the issue link and, for the owner, Approve. Other proposed tasks stay in Proposed. The Proposed tile still counts both. The `TaskRow` gained `issue` and `issue_url` (the github.com page, only when the API is GitHub's own), `HeaderView` gained `pending_issues`; both additive, `PROTOCOL` stays 1.
- **No store migration, no event, no state change.**

## Not verified

Against a real GitHub (only recorded responses and the fake host), and the monitors on screen (their logic is tested through conformance items and unit tests, not drawn and looked at).

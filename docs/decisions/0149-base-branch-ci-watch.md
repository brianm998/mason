# 0149: The base branch's own CI is watched, and a red gate freezes landing

MASON ran its own suites on a moved base (the post-land tier) but never looked at what the CI host says about the base branch itself. Star's starpipe does (`watch_develop`), and the owner wants no logic in two places, so MASON takes it over. It is off unless a project's definition turns it on, so the commit that turns it on can turn the old watcher off.

**Where it is switched on: `[watch.ci]` in `.mason/mason.toml`.** Read from the base commit like every other part of the definition, so turning it on is a reviewed change. Without the section, or with `enabled = false`, the daemon makes no call to the CI host, files nothing and sets no freeze. Keys (an unknown key is refused with a "did you mean" hint):

| key | default | meaning |
|---|---|---|
| `enabled` | `false` | the switch |
| `workflows` | all | workflow names to watch (the run's name) |
| `exclude` | none | workflows whose baseline is kept but are never filed and never freeze (star: `["Gherkin"]`, its nightly owns those) |
| `gate` | none | `"workflow"` (every job in it) or `"workflow/job"`: a red acts at once |
| `agreement` | `2` | distinct red runs, with no green between, before a non-gate job is filed |
| `red_ticket_after` | `"3h"` | a job red this long with no open ticket naming it is filed |
| `max_open_auto` | `2` | auto-filed tasks open at once; past it a red is filed `proposed` |
| `freeze_on_gate` | `true` | a gate red freezes the project's landing until that job is green on the base |

The repository and token are not repeated: they come from the project's `[ci]` section, which must be `github-actions`. With `[watch.ci]` on and another kind of `[ci]` (or none) the watch cannot run, says so in `mason status` and the monitors' banner, and does nothing else. This cannot be checked when the definition is read, because `[ci]` lives in the machine's project file, not on the base branch.

**The rules** (ported from starpipe's behaviour, not its code) are in `mason_core::ci_watch`, with no I/O and the clock as a parameter:

- *Seeding.* A workflow not seen before is seeded from its newest finished `success` or `failure` run: each job's baseline is set and nothing is filed. A job already red is the known-red ledger; it is not agreed into a ticket. (It can still be filed by the long-red rule, counted from when it was first seen.)
- *Passed over.* Runs that ended any other way, and jobs `skipped` (or `neutral`) inside a finished run, change nothing.
- *Infrastructure.* A job that ended `cancelled`, `timed_out` or `startup_failure` (or `action_required`, `stale`) is passed over: never a red, never a green.
- *Gate.* A gate job red after green (or never seen before) files at once, urgent, on that one run.
- *Agreement.* A red after green is pending with its run id; a red on another run, with no green between, reaches `agreement` and files. A green clears it. The same run seen twice counts once.
- *Long red.* Red for `red_ticket_after` with no open ticket naming it files; then it is looked at again only after another threshold (the clock restarts at the filing, whether or not the ticket was made).
- *Dedupe.* No filing while an open MASON task's title, or an open tracker issue's title or body, contains `workflow/job`. The gate still freezes in that case, on the task that names it.
- *Cap.* The tasks the watch filed that are still open are counted; at `max_open_auto` the next red is filed `proposed`.

**What is filed.** `<workflow>/<job> red on <base>`, owner trust, normal priority (urgent for the gate), a body with the run link, the sha, why it was filed and the last 60 lines of the failed job's log. The log is best effort: GitHub answers with a redirect to a signed address, which is read without the token; if it cannot be read the task says so.

**The freeze: its own source, in the existing row.** A project has one freeze row. `landing.rs` (protected: guards) sets it when the post-land tier is red and *clears it whenever a post-land tier comes out green*, which would lift a CI freeze on any unrelated green. Rather than edit `landing.rs`, the store's freeze row carries a `source` (`post-land` or `ci`, migration 25). `set_freeze` and `clear_freeze` (what `landing.rs` calls) act only on `post-land` rows and never replace a `ci` row; `set_ci_freeze` and `clear_ci_freeze` do the same for `ci`. Landing already reads the row and lets only `fix_task` through, so a CI freeze needs nothing else there. The watch re-asserts its freeze on every pass while a gate job is red, so a post-land freeze that is cleared while the gate is still red gives way to it at the next look. If both are wanted, the one that came first is the one shown. The fix task is the task the watch filed (or found naming the job). If the watch is switched off, or `freeze_on_gate` is, the freeze it holds is lifted at the next pass.

**State** is one row per project (`ci_watch`, migration 25) holding the baselines, the pending reds and what was filed, as text the daemon writes. It is stored only after a pass has done everything it decided, so a failed filing is repeated; the repeat finds the task already named and files no second one.

**Shown** in `mason status` and `/status` (`ci_watch`: workflows watched, gate, exclusions, reds waiting for agreement with their count, what was filed, the freeze, why it cannot look), and as banners in the header both monitors render from: the freeze (its wording says it lifts on a green base) and the reds waiting for agreement. No new protocol field was added, so no client changes and no `clients/features.toml` entry.

**Not decided here / left.** The alert for a frozen base (`attention.rs`, protected) still says "the freeze will not lift by itself ... `mason task force`" when the fix task has failed; for a CI freeze that is not the way out (a green run is). Changing it is for the owner. Jobs in a workflow's matrix are keyed by their job name as GitHub reports it.

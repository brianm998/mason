# 0206: A task sent back or retried is repinned to the project's current definition

Amends 0015 (a task stays pinned to the definition it was admitted under).

**Why.** 0015 pins a task so that a running step finishes on the version it started with. That is
right inside a step. Across a send-back it is wrong when the definition itself was the fault: on
2026-11-12 star's `build-*` suites had only a `fail_regex`, so a clean build counted no tests and was
red. The fix reached star's `develop` at 12:53; tasks admitted earlier stayed pinned to the broken
suites until 18:33 (10 send-backs), and agents edited `.mason/mason.toml` on their own branches,
which cannot change the pinned definition.

**Decided.**

- **Repin at the boundary.** When a task goes back to an earlier step (a send-back for red, review
  changes, an owner's rejection or a base conflict, or `mason task retry`), it is first pinned to the
  project's current definition, and a `Repinned` event records the old hash, the new hash and the
  steps and suites that differ. A step that is running keeps the definition it holds; the new pin
  is what the next step starts under. If the current definition has no step to go back to (the
  step is gone, or no longer has an `on_red`), the task keeps its pin and goes back as before. A
  retry resolves its target step in the current definition, and keeps the pin if the step is not there.
- **A red in a suite that changed is rerun once.** When a test step ends red naming a suite, and
  that suite differs in the project's current definition from the one the task is pinned to
  (changed, added or removed), the step is run again in the same step run under the current
  definition, instead of sending the task back. It is the same run, so the same round: `max_rounds`
  still bounds it. It happens at most once per step run (`def-reruns.txt` in the task's directory).
  Which suite is red is read from the reason the test step already gives (``suite `x` is red`` and
  the build form); a red with no suite (a gate, a reviewer) is sent back as before.
  This lives in `jobs.rs`; `suites.rs` (protected) is unchanged.
- **A note, not a block, for `.mason/` changes.** On a project without `[self]`, a develop prompt
  for a task whose diff touches `.mason/` says that the pipeline runs the base's definition and
  that `.mason/` changes need a task of their own. Nothing is refused.
- **Admission is unchanged**: a new task is pinned to the definition in force when it is admitted.

**Not decided here.** Repinning a task that is merely waiting (no send-back) or at a plain step
success: it keeps its pin until it goes back.

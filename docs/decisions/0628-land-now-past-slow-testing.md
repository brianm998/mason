# 0628: Land now stops a running batch's remaining testing, keeps the freeze and the build floor, and owes the tests after the merge

Status: waits for the owner's approval (it changes what verification a batch may skip, in `landing.rs`, `suites.rs`, `attention.rs`-adjacent debt payment and `clients/features.toml`, all in `PROTECTED`).

**Decision.** Land now on a task or batch that is already running stops the batch's remaining local suites and CI wait. It is not force: it still respects a landing freeze (a non-fix task does not land while frozen), still needs the batch to have merged cleanly and passed the build floor, and the whole batch lands together. The tests it stopped are not dropped: each member gets one deferred debt per stopped suite (`suite:NAME`) and for CI (`ci:<workflow>`), which the existing post-land payment runs on the base tip; a red result is an owed debt and raises the existing alert.

**Why not reuse force.** Force skips the freeze, the build floor and CI, and records debts that cannot be paid. Land now is meant for "this is slow and I accept the risk of testing it after the merge", not "ignore every gate".

**Why debts are recorded at promotion, not when the button is pressed.** A debt named `suite:NAME` is also a skip for that task's own testing. Written early, a batch that then failed its build floor would send its tasks back with their testing silently skipped. They are written only once the batch has landed.

**Consequences.** `landing:force` / `landing:land_now` debts now resolve to the batch tier's suites, so they are paid after the merge as well. A task pressed Land now while queued still gets its own batch and is verified as before (nothing was skipped, so no suite debt). PR modes are unchanged.

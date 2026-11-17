# 0514: landing runs a lint tier on every batch

**Decision.** `.mason/landing.toml` may declare a tier named `lint`. Landing runs the `command` of each suite it names over every batch, tests skipped or not, and again when the base moves. A red parks the batch with the lint output's files and lines in the note.

**Why.** Six lint errors and five unformatted files landed on develop in two days: mason has no batch tier, and the rebuild of a skipped batch (s429) only compiles. A separate tier keeps the build floor and the rebuild-only rule for tests.

**Why not a suite in the test step.** The owner's skip of the test step skips its suites, which is the case to cover.

**Trust.** `crates/daemon/src/landing.rs` and `.mason/**` are in `PROTECTED` (gates). This adds a gate and waits for the owner's approval.

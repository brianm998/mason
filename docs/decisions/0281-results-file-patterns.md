# 0281: `results_file` may end in a wildcard

**Decision.** A suite's `results_file` whose last segment has `*` or `?` names every matching file in that directory; MASON reads them as one set of results and removes them all before a run. A plain path behaves as before.

**Why.** Tools such as Gradle write one results file per test class. Joining them in the suite's shell command (as mason's own `compose-clients` does) works, but a definition that names the pattern directly was silently never green: star's `test-desktop` failed every passing run with "was not written".

**Trust.** `crates/daemon/src/suites.rs` (verdict logic: running suites) and `scripts/exit/s286.sh` (exit tests) are in `PROTECTED`; the owner approved this change on 2026-11-14 ("Yes, all three"). `suites/base_red.rs` is not. No gate is weakened: no match is still "not written", and every matching file's failures count.

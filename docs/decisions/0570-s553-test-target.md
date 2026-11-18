# 0570: Point the s553 exit test at the `it` harness

Context: `crates/daemon/tests/base_red.rs` moved to `crates/daemon/tests/it/base_red.rs`, a module of the `it` target, so `--test base_red` no longer exists and CI/exit-scripts failed at s553.

Decision: run `cargo test -p mason-daemon --test it a_skip_with_no_step_name`. The same single test runs and the grep guard stays; no gate is weakened.

Trust: `scripts/exit/**` is in PROTECTED (`crates/daemon/src/protect.rs`), so this edit to s553.sh and the new s577.sh need the owner's approval.

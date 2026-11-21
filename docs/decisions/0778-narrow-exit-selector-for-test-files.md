# 0778: narrow the exit selector for test files

Status: accepted

For a changed file under `crates/<c>/tests/`, `select-exit-scripts.sh` matches scripts naming the
module stem (whole word; `mod.rs`/`main.rs` use the directory name), plus the existing full-path match.
It no longer matches `-p <crate>` or `--test <target>`.

Trade-off: a shared helper such as `tests/it/common.rs` is matched only by scripts naming `common`.
Compile breaks in integration tests stay covered by the cargo-workspace suite, which builds and runs
the workspace.

Open follow-up: an edit under `crates/daemon/src` still selects ~225 scripts via `-p mason-daemon`,
which can still exceed the runner limit. Needs a cap or sharding.

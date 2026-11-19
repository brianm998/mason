# 0627: per-test times come from libtest's `--report-time`

Status: accepted (s634).

Context: `it` is one executable of 543 tests; libtest's summary gives one number, so slow tests are invisible.

Decision: run the cargo suite with `-- -Z unstable-options --report-time`, and give `RUSTC_BOOTSTRAP=1` only to the processes that parse those options: the test executables through a cargo `runner` (`env RUSTC_BOOTSTRAP=1`) and the doc-test harness through a `RUSTDOC` wrapper. Never give it to cargo or rustc, because `anyhow`, `thiserror` and `proc-macro2` rerun their build scripts when it changes, and a recompile inside the timed run made the suite run out of time twice. In-test `cargo build` calls remove it. Then read the `<N.NNNs>` suffix in the regex parser, and keep the median of each test's last 10 times in `test_times`. Both whole and narrowed runs record.

Consequence: the `.mason/mason.toml` suite command and the `crates/testing` parser change, both trust files needing owner approval. `RUSTC_BOOTSTRAP=1` is an unstable-option switch; if a toolchain stops accepting it, result lines lose the suffix, the regex still matches them, and the list just stops updating. Tradeoff: the share is of the suite's recorded tests, not of one executable.

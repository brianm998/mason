# 0590: exit scripts name daemon tests by their `it` module

Exit scripts run daemon integration tests as `cargo test -p mason-daemon --test it <module>::` and grep `crates/daemon/tests/it/<module>.rs`. A script that names a standalone test target folded into `it` fails on every OS, and the ubuntu shard shows it. Extends decision 0560.

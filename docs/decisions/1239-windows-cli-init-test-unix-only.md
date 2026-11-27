# 1239: cli init test is Unix-only

Decision: gate the whole of `crates/cli/tests/init.rs` with `#![cfg(unix)]` rather than only the chmod. Why: every test depends on `#!/bin/sh` stubs, `chmod`, `python3` and Unix PATHs, so a partial gate would still fail at run time. This follows `crates/adapters/tests/codex.rs`. No gate, guard or budget changed.

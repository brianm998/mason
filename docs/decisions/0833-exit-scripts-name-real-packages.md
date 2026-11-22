# 0833: Exit scripts name real packages

Exit scripts name cargo packages as spelled in `crates/*/Cargo.toml` `[package]`. The CLI crate is
`mason-pipeline`, and its only target is the `mason` bin, so its unit tests need `--bin mason`, not
`--lib`. A script that needs a test binary's path reads it from `--message-format=json`
(`"executable"`), not from `target/` paths in `-q` output, because the target directory differs on
GitHub. `scripts/check-exit-guards.sh` refuses `-p NAME` for a NAME that is not a package. A fixture
line may carry `guard-ok`.

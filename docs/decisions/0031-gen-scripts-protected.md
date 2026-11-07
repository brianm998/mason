# 0031: scripts/gen-*.sh are protected (s53)

`crates/daemon/src/protect.rs` is a trust file: this change waits for the owner. It adds a protected path and narrows nothing; no entry of `PROTECTED` or `ADDED_OK` was removed or changed.

Why: `Batch::regenerate_status` (`crates/landing/src/batch.rs`) runs `bash scripts/gen-status.sh` from the integration worktree, with the daemon's environment and outside the step sandbox. Any task in the batch could edit it unseen.

## Audit: what the daemon runs from the repository

Searched `Command::new(` and `"scripts/` in `crates/daemon/src` and `crates/landing/src`.

| Run by | What | Protected before? |
|---|---|---|
| `batch.rs` `regenerate_status` | `scripts/gen-status.sh` | **No** — now `scripts/gen-*.sh` |
| exit tests / self-test | `scripts/run-exit-scripts.sh` | Yes (own entry) |
| exit tests | `scripts/exit/**` | Yes (`ADDED_OK`: a new file is not held by path, its content is scanned) |
| repository checks | `scripts/check-*.sh` | Yes |
| `landing.rs` | `.mason/landing.toml` (test timeout, resolver, suites) | Yes (`.mason/**`) |
| `defs.rs`, `prompts.rs`, `plugins.rs` | `.mason/mason.toml`, `.mason/prompts/**`, `.mason/extensions` | Yes (`.mason/**`) |
| `jobs.rs`, `batch.rs` resolver, `update.rs` | `sh -c <cmd>` where the command comes from the owner's project file or `.mason/landing.toml`, not from a repo script | n/a (the config is covered above or is owner-held) |

Found but not added: `scripts/gen-clients.sh` matches the new glob but the daemon does not run it (it is now held too, which is harmless). `scripts/bundle-monitor.sh` is not run by the daemon, so it was not added.

Not verified: `update.rs` builds with a command from configuration; I did not trace whether that command in a given deployment calls a repository script.

# 0173: A suite may build first; MASON's own test step runs suites chosen by path

Every change ran `cargo test --workspace` in `run_check` and again in the test step, even one that touched only the clients, and never built Swift or Kotlin. Decision 0130's build/test split only existed for a step's single command.

**`build` on a suite.** It runs first holding one `compile` slot, which is given back; the suite's command then holds the step's `uses` without `compile`, plus the suite's and its environment's own. A suite without `build` holds the whole step's `uses` as before. A step that has any selected suite with a `build` queues suite by suite instead of holding everything for the whole step. Suites with an identical build command share one build per tree; a suite already green for the tree is not built. `build` on the step itself, with suites, is still refused.

**`run_check` on a suites step.** The suites bound to `development` that the impact map selects for the worktree's changes run, one report line per suite. With no `development` binding, today's single command. An unmapped path selects everything (the impact map's own rule); nothing selected passes and says so.

**`{project_test}`.** The project file's `test` command (with overrides) is substituted into a suite's `command`, `build` and `rerun`. The owner's per-worktree `CARGO_TARGET_DIR` lives in that command, so it applies without a path in the repository. A suite that uses it in a project with no `test` is a red that says so.

**MASON's suites.** `cargo-workspace` (`{project_test}`, regex-parsed from cargo's lines; its build would be `{project_test} --no-run`), `swift-clients`, `compose-clients`; `crates/**`, `Cargo.*`, `rust-toolchain.toml` select the first, `clients/swift/**` and `clients/compose/**` their own, `clients/features.toml`, `clients/tokens.toml` and `schemas/**` all three, anything else everything. A change under `crates/**` runs exactly today's workspace command; a change under the client paths adds Swift and Kotlin checks that did not run before.

**Docs-only changes: proposed, not taken.** Running no suite for `docs/**` and `**/*.md` would relax today's gate (the workspace has tests that read docs and scripts), so the map keeps `cargo-workspace` there. To relax it, the owner changes the two lines to `= []`; a change selecting nothing then closes `untested`.

**Order of landing.** A change to `.mason/mason.toml` is validated by the daemon running when it lands, which does not know a suite's `build`. So the three suites land with their `build` lines commented out, and a follow-up uncomments them once the daemon with this change is live.

**Not done:** see docs/stages/s178.md (Swift and Gradle commands unrun; restart mid-build restarts the build).

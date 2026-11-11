# 0122: A test step may build before it tests

A workspace test run held one `compile` slot from its first compile to its last test. Most of that time it launches processes and waits on disk, so CPU reads idle while builds queue behind it (2026-11-11: tasks waiting for compile on an idle machine). The starry pipeline keeps build slots and a test slot apart; this does the same.

**`build`, a test step's optional second command.** It is set on the step in `.mason/mason.toml`, or as `build` beside `test` in the project's overlay, or overridden (`step.<test>.build`). It goes only with a step's one command: suites build in their own commands, so `build` with `suites` is refused.

**Two phases, each queued on its own.** The build runs first, holding one `compile` slot whatever `uses` says. Then the slot is given back, and the tests run holding `uses` without `compile`. A failed build is red with its output as feedback; the tests do not run, and test reruns (`test_reruns`) do not rebuild. `run_check` (host `t<task>r<run>-checkbuild<n>`) and landing's re-test (`b<batch>-rebuild<n>`) run the same two phases. A daemon that restarts and finds a phase's host already started collects it without queuing, as `hostedcheck` already does.

**Why `uses` minus `compile`, not a new key.** `uses` stays what the tests hold, and `compile` is the build class by definition. A project that wants test runs capped names a class (`test-suite = 1`) in `uses`. Without `build`, nothing changes.

**Not done:** the test step's command is assumed not to rebuild much after `build` (for cargo, `cargo test` after `cargo test --no-run` only checks fingerprints). A test command that does compile a lot would do so without a slot.

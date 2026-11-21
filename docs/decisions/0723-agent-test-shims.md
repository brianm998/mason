# 0723: with remote on, an agent's own test commands are refused

Status: accepted (#704, s730)

**Decision.** When a task's checks go to GitHub (its `run_on` choice, else the global switch, for a project with `[remote_runs]`), the daemon writes wrappers for `cargo`, `swift`, `xcodebuild`, `gradle` and `gradlew` into `<data dir>/shims` and puts that directory first on the develop and review agent's PATH. A wrapper refuses test forms with exit 69 and "Tests run on GitHub for this task: call run_check ..." and execs the real tool (absolute path resolved at write time) for everything else. `MASON_REMOTE_SHIM=off` disables them.

**Why.** An agent's shell runs on this Mac whatever the switch says; a whole-workspace test there heats the machine and is not evidence. Builds stay local because an agent cannot iterate through a GitHub round trip.

**Trust.** The develop prompt line is appended in `jobs.rs`, not in the protected `prompts.rs`; no gate, limit or verdict changes.

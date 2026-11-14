# 0306: MASON's own checkouts run the worktree setup

**Decision.** The worktree setup is the command steps a task passes before its first change: from `start` and `explored_entry`, past exploring agents, up to the first step of another kind. Every checkout MASON makes for itself runs them before it builds. There is no new definition key.

**Why.** A project that links gitignored artifacts into a worktree already says how, in a step. Reusing that step keeps one source of truth. A new key would have to be kept in step with the step, and star's `.mason` would need changing. A base checkout without the artifacts fails to build, and the build error is cached as the base's profile, so a red that is already on develop is never pinned there (star #259, 2026-11-14).

**Trust.** `crates/daemon/src/suites.rs`, `crates/daemon/src/landing.rs` and `scripts/exit/s312.sh` are in `PROTECTED`. The owner approved this change on 2026-11-14 ("Yes, fix it here"). Nothing is weakened. A base whose setup fails is unknown rather than red, so it pins nothing on the base and excuses nothing.

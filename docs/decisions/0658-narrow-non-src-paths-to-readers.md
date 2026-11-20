# 0658: a path outside src/ selects the targets that read it, not the workspace

Status: accepted (#652, s665).

**Context.** Any changed path that was not a crate's `src/` or a test target's file made the test-target picker run every test (2,169 tests, ~38 minutes) although most such files are read by one or two targets or none.

**Decision.** Only paths that can reach every target keep the whole run: root `Cargo.toml`/`Cargo.lock`, `rust-toolchain*`, `.cargo/**`, and paths the suite's commands name. Everything else selects by static text match (package whole for a manifest/build script, support files by directory name, data files by file name). An undecidable reference includes the target; a path nobody names selects nothing of its own. The reason a run is whole is carried in `Pick::All` and printed.

**Consequence.** A file read by a mechanism the text scan cannot see (a path built at run time) is not caught at the task's test step; the landing batch still runs everything, so it is caught before `develop`.

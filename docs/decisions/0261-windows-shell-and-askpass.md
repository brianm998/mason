# 0261: project commands run through one shell function; askpass is a `.cmd` on Windows

**Decision.** `mason_platform::shell` returns the shell for project commands: `sh` on Unix; on Windows Git for Windows' `bash.exe`, found from `MASON_SHELL`, `git --exec-path`, the `Program Files` folders, then PATH (never `System32`, whose `bash.exe` is the WSL launcher). Missing, it fails with a message naming Git for Windows. Every `sh -c` in `jobs.rs` and the adapters' `run_command` goes through it. The askpass helper on Windows is `packaging/mason-askpass.cmd` (a PowerShell dialog), which `askpass_script()` looks for by platform.

**Why.** A `.cmd` was chosen over a `mason askpass` subcommand because the Unix helper is already a standalone script the daemon points `SSH_ASKPASS` at, and a script needs no argument handling by the CLI. W6 packages the `.cmd` beside `masond`.

**Trust.** No `PROTECTED` path; `scripts/exit/**` stays bash. This branch also closes merge-conflict markers left in `crates/daemon/src/landing.rs` on develop (both functions kept), without which the daemon does not compile.

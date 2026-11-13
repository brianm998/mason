# 0188: Windows is a target for the daemon and CLI, not only for clients

Supersedes the line in `docs/DESIGN.md` (Stack) that read "The daemon targets macOS and Linux
(launchd and systemd). Windows is limited to clients for now."

The owner wants the daemon and CLI released for Windows, as Linux, macOS and Windows packages from
one tagged release. Before this record no crate built there: `nix` was an unconditional dependency of
seven crates and 28 source files used Unix-only APIs.

**Decision.**
- Windows is a supported target for the whole workspace. The macOS and Linux behaviour does not change.
- The OS differences live in one crate, `mason-platform` (`crates/platform`): process groups and signals,
  unix sockets, `exec`, positioned reads, permission bits, disk space and the host name. Each function is the
  same `std`/`nix` call the callers made before on Unix, and a `cfg(windows)` counterpart.
- `nix` is a `cfg(unix)` dependency only. File locks use `std::fs::File::try_lock`, which is `flock` on Unix
  and `LockFileEx` on Windows.
- A Windows piece that belongs to a later port task returns an `Unsupported` error that names the task
  (IPC transport, process trees, daemon takeover, service, shell); it is never a silent no-op. Pieces done in
  the port's first task: free disk space (`GetDiskFreeSpaceExW`), process liveness (`OpenProcess`), the open-file
  limit (nothing to raise), file permissions (none; the data directory is per user), the host name.
- `windows-sys` (MIT OR Apache-2.0, already in `Cargo.lock`) is the only new dependency, for the two calls above.
- CI gets a `windows` job (`windows-2022`) that builds and runs clippy. Tests do not run there until the port
  tasks land; the Unix-only tests are gated with `cfg(unix)`.

**Trust.** Protected files touched, all to make them portable and none to change a verdict, gate or limit:
`.github/workflows/ci.yml` (the `windows` job), `scripts/exit/s193.sh` (new), `crates/adapter-api/src/contract.rs`
(one `use` line: the socket types now come from `mason-platform`, which re-exports the `std` ones on Unix, so the
Unix behaviour is the same code), `crates/daemon/src/update.rs` (`exec` goes through `replace_process`, which is
`CommandExt::exec` on Unix), `crates/landing/**` (`proc.rs` process-group kill; Cargo.toml; two test files gated
`cfg(unix)`) and `crates/resources/**` (`flock` through `std::fs::File::try_lock`, the same `flock(LOCK_NB)` call).

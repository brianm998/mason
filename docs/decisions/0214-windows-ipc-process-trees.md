# 0214: Windows step-host IPC is AF_UNIX; process trees are named Job Objects

Builds on `0188-windows-port.md`. Step hosts talked to the daemon over Unix domain sockets and were stopped
with `killpg`; neither existed on Windows.

**Transport: `AF_UNIX`, not a named pipe.** Windows 10 1803 and later (and Windows Server 2019) support
`AF_UNIX` stream sockets through Winsock. A step host's endpoint is the path `<dir>/sock` on both systems, so
everything that treats the endpoint as a file keeps working unchanged: `host.kv` records the path, the launcher
waits for `info.sock.exists()`, the daemon finds live hosts after a restart by reading the registry, and a
connect needs only the path. A named pipe (`\\.\pipe\mason-step-host-<id>`) would have needed a second kind
of endpoint in `host.kv`, the launcher's readiness check, the registry, the tests, and the contract socket and
MCP bridge (which also use `mason_platform::net`). Cost: `std` has no stable `AF_UNIX` type on Windows, so
`mason_platform::net` implements `UnixStream`/`UnixListener` over Winsock with `windows-sys`
(`Win32_Networking_WinSock`), with the methods the callers use (`bind`, `connect`, `accept`, `try_clone`,
`set_nonblocking`, read/write timeouts, `shutdown`). On Unix they remain the `std` types. A socket read
timeout surfaces as `WouldBlock`, as on Unix. Paths must be UTF-8 and under 108 bytes. Older Windows is not
supported.

**Process trees: one Job Object per started child.** `mason_platform::process::spawn_group(&mut Command)`
replaces `group_leader` + `spawn` at every site. On Unix it is exactly that. On Windows it spawns, then creates
a Job Object named `mason-job-<pid>`, assigns the child and closes its handle. Kill-on-close is not set, so a
step host and its tree survive a daemon restart as on Unix; a job lasts until its last handle is closed and
its processes have exited. `signal_group(pid, ...)` opens the job by that name from any process and calls
`TerminateJobObject`, which is how the host, the registry's reap, `watch.rs` and `claude.rs` stop a tree. If
no such job exists (never made, or all gone) it falls back to `TerminateProcess` on the pid. Windows has no
signals: `Term` and `Kill` both terminate at once (so the host's grace period is moot), and `Cont` is
`Unsupported`. The step host is launched with `CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP`
(`process::detached`) in place of `setsid`.

**Registry identity on Windows.** `proc_start` is the process creation time (`GetProcessTimes`), `proc_command`
the executable path (`QueryFullProcessImageNameW`; another process's command line is not cheaply readable, so
`reap_dir` checks `step-host` in the path but not the directory there; the creation time pins the process),
and `proc_pgid` is the pid for a live process, because `spawn_group` makes each child its own job.

**Known limits.** A grandchild created in the instant between spawn and job assignment is outside the job.
If MASON itself runs inside a job that forbids breakaway and nesting, assignment fails and only the child is
terminated. Job names are per pid; a stale job whose processes outlive a reused pid would be joined by the new
process.

**Trust.** `.github/workflows/ci.yml` (the `windows` job now runs the platform, step-host and adapters tests),
`crates/landing/src/proc.rs` (`spawn_group` in place of `group_leader` + `spawn`), `scripts/exit/s219.sh` (new).
Protocol negotiation is not touched. New Cargo features on the existing `windows-sys` dependency only.

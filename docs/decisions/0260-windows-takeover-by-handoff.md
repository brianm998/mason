# 0260: Windows takeover is a handoff, and the managed loader supervises a child

**Decision.** Windows has no exec. The old daemon starts the new runtime with the same arguments and `MASON_TAKEOVER_FROM=<its pid>`, then exits with code 75. The new daemon waits (by condition) for that pid to be gone, then binds the same port. `masond --managed` stays running as a loader: it follows a handover (code 75) to the successor, ends on a clean exit, and on a crash picks the runtime again, so the crash counting and rollback (decision 0013) still apply. Unix keeps exec.

**Why not "wait until the new one has the port, then exit".** A listening port is exclusive on Windows (`SO_REUSEADDR` there allows hijacking), so the new daemon cannot bind while the old one lives. If the successor fails to start serving, the managed loader's boot record counts it and rolls back; without the loader the daemon stays down, as after a failed start.

**Runtimes.** A runtime directory is written once, before it is current, and never again; `current` changes by atomic write. A running .exe is never replaced.

**Trust.** No PROTECTED file changed. `selftest.rs` is called as before; `ci.yml` was left alone, so enabling update tests on the Windows job awaits the owner.

# 0262: Windows service is a per-user logon Scheduled Task; CPU gate reads GetSystemTimes over an interval

**Decision.** `mason service install|uninstall|status` on Windows registers a Scheduled Task `MasonDaemon` for the current user, triggered at logon, running `masond.exe --managed` with least privilege (no admin rights, no service account). The task definition is written as XML to `%USERPROFILE%\AppData\Local\mason\masond-task.xml` (it carries the same "written by" marker, so refuse-to-overwrite, `--force` and `--print` behave as for launchd/systemd) and registered with `schtasks /Create /XML ... /F`, then started with `/Run`. It restarts on failure (1 min, up to 999 times).

**CPU reading (owner to confirm).** Windows has no load average. The primary reading is `GetSystemTimes` sampled twice 250 ms apart: busy% = 1 - idle/(kernel+user) over the interval (kernel time includes idle). This is the same shape as the Linux `/proc/stat` and macOS host_statistics readings, so the gate keeps its meaning: work waits when the machine is saturated. `read_loadavg` returns `None` on Windows (no fallback), and the sampler already treats that as "absent". Unix readings are unchanged. `limits.kv` keys are unchanged.

**Memory.** `GlobalMemoryStatusEx.ullAvailPhys` in MB.

**Trust.** No PROTECTED file touched. The Windows CPU reading is a new gate input and needs the owner's sign-off.

**Not verified.** Nothing here was compiled or run on Windows (this machine is macOS). The `windows-sys` calls and `schtasks` behaviour (including that `schtasks /XML` accepts a UTF-8 file with a UTF-8 declaration) are unverified; only the pure parts (task XML, runner command lines, tick arithmetic, `schtasks` output parsing) are tested here. `mason service status` for the task treats `Status: Running` in `/Query /FO LIST` as running; this is localised on non-English Windows.

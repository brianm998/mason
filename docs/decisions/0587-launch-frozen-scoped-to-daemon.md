# 0587: launch-frozen is scoped to the daemon's hosts root

The launcher's list of launches in progress is process-wide, so in a process with several daemons (the merged `it` test executable) one daemon's frozen launch raised `launch-frozen` in all. Each launch is now registered with its hosts root and a daemon asks for `launching_under(cfg.hosts_dir())`. The threshold moved to `DaemonConfig::launch_frozen`, still overridden by `MASON_LAUNCH_FROZEN_MS` when the config is built. `launching()` stays for callers that want the whole process.

# 0522: disk space history is kept per physical volume

**Decision.** The daemon records free and total bytes of each volume holding MASON's data, caches, projects or worktrees, every 30 s, and serves them as `DiskHistory` (HTTP `/disk/history`, view `DISK_HISTORY`). A volume is keyed by device id; minute buckets keep the last reading, since free space is a level.

**Why.** The monitors showed CPU only, but the disk is what stalled MASON before (USB-disk freezes, a full build volume), and the floor checked only the data dir and caches, not where the projects live.

**Why one path function.** The floor, the stall prober and the history use `disk::relevant_paths`, so they cannot disagree about which volumes matter.

**Not grouped by APFS container.** Two APFS volumes of one container show as two entries; telling them apart needs `f_mntfromname`, which would need `unsafe`.

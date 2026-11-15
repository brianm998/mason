# 0431: the monitor update body carries the running build

The monitor's `/update/monitor` body carries `own_path` and `own_version` besides `kind` (s326),
so the daemon can rebuild in place. Tests parse the JSON and assert those keys instead of
comparing the exact string, so adding a field does not break them.

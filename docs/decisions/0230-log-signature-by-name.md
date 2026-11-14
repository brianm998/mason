# 0230: the log cache signature finds a task's hosts by directory name

`log_signature` is called on every task-page build. It now uses `hostarchive::task_hosts` (prefix `t{task}r`, live then archive) rather than scanning all hosts, so cost follows the task's hosts, not the registry's size. Archived hosts are included because `render_logs` includes them. No gate, limit or trust file changed.

# 1088: s798 red on Ubuntu

The 240 s deadline in `s798` is not raised. The check was not slow: the agent never wrote the test ([stage note](../stages/s1099.md)), so no wait would have been long enough. No assertion is loosened (decision 0755).

Instead the first failure line carries the task state and the last log line. The runner's summary keeps only the first matching line, cut to 200 characters, and that summary is what a filed red-CI task shows. The state comes first so that the cut falls on the log line.

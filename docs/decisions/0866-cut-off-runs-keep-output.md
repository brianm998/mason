# 0866: a cut-off remote run keeps its output

Decision: on the limit, MASON cancels, then waits at most 180 s for the artifact and keeps its `output.log` before the cut-off line. The workflow's `run` step gets `timeout_min` = limit - 2 min through run.json, so no workflow edit is needed. Unfinished tests are named from libtest's "has been running for over" lines.

Why: #760's hung check left a one-line log. Not verified against a live run.

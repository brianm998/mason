# 1163: one sentence for a held queued task

Decision: `Daemon::queued_wait_text` is the only place the check-queue and file hold are described;
the CLI and monitor views call it. Display only: no hold, rank or verdict changes.
Why: the monitor built its own sentence without asking the holds and blamed LLM sessions.
Lapse is shown as time left, not wall-clock time (no local-time formatting available).

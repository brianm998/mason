# 0694 — CI-watch quotes the error, and skipped/neutral clear a red

Decision: file tickets with an excerpt of the failing step (log cut at the first `##[error]`, marker lines plus context) rather than the last 60 lines, which are post-job noise. Treat `skipped` and `neutral` job conclusions like `success`. `cancelled` and `timed_out` handling is unchanged.

Consequence: a job skipped because a `needs:` dependency failed also clears its own red; the dependency's red is still tracked on its own.

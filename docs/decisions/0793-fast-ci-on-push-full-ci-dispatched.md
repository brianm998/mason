# 0793: develop CI on push is the fast jobs; the full matrix is dispatched by MASON

Every push to develop started about 23 jobs (370 runner-minutes) that shared GitHub's 20-job
account limit with MASON's remote runs, after every batch had already run the whole cargo suite.
The owner chose (2026-11-20) to keep only fast checks on push and run the full matrix from MASON's
full-test schedule (decision 0775).

Decision: push runs `lint` and `windows`; `workflow_dispatch` (run-name `CI: full`, input `macos`)
and pull requests run the rest; the GitHub cron is removed. The CI watch keeps one state per job
and treats a job as absent only from runs of the kind it last appeared in (push or full), so a job
missing from a push run is neither stale nor waiting. A fix for a full-only job waits for the next
full run. `.github/workflows/**` is protected: the change waits for the owner's approval. The
schedule block is an example for the owner's own project file.

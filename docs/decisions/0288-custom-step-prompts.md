# 0288: custom step prompts and round packs

- A step's `prompt` file is required: if it is missing, empty or too big the step fails naming the file. Falling back to the built-in prompt would run a different job.
- Fix, conflict and CI-fix rounds are develop rounds, not new session kinds. The daemon records the reason in `round-reason.txt` next to `feedback.txt`, so a stale marker cannot outlive its feedback.
- Fallback for a fix round is `fix.md`, then `develop.md` (existing projects keep their rounds), then the built-in prompt.
- The editor edits the file name only, not the file contents.
- `ci-fix` is left for when the landing change is approved.

# 0314: The daemon names the resume command; the CLI only runs it

The CLI hard-coded `claude --resume`, which fails for a Codex project. The daemon knows the project's provider, so `GET /tasks/{id}/session` returns `provider`, `bin` and `argv`, and `mason task open` runs that argv in the worktree. The old `claude` field stays for older CLIs, and the CLI falls back to it when `argv` is missing. Not a gate or trust change.

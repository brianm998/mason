# 0787: redo 0782 — blame coloured lint and build reds by file

Status: proposed (verdict logic in landing.rs; owner approval needed)

Decision: as 0782, now actually implemented (s794): strip ANSI before parsing, parse the full output rather than the 40-line tail, ask remote runs for no colour in `run.json`, and route the build floor through the same file blame. Splitting stays the fallback when no file maps to a task.

Why: 0782's code never landed (a failed text replace, nothing built), so batch #506 still split on a coloured clippy error.

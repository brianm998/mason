# 0782: blame coloured lint and build reds by file

Status: proposed (verdict logic in landing.rs; owner approval needed)

Decision: strip ANSI before parsing, parse the full output rather than the 40-line tail, ask remote runs for no colour in `run.json`, and route the build floor through the same file blame. Splitting stays the fallback when no file maps to a task.

Why: GitHub's coloured `--> ` lines defeated `lint_files`, so a lint red was bisected at 5-7 min a round.

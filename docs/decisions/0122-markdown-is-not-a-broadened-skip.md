# 0122: A Markdown line is never a broadened skip

**Decision.** The weakened-check scan (`crates/daemon/src/protect.rs`) no longer reports `broadened-skip` for lines added to `.md` or `.markdown` files. All its other findings, and every script, workflow and config file, are scanned as before.

**Why.** Task #73 (2026-11-11) replaced an or-true counter in `scripts/exit/s98.sh` with `find`, then wrote a stage note that quoted the old shell. The scan flagged the note, `docs/stages/s98.md:19`, as a broadened skip. The agent read that as "restore the check" and put the or-true back, and the task failed after three landing rounds. Prose runs nothing, so a quote in it cannot loosen a check, and the false finding pushed the agent to make things worse.

**Trust.** This changes the self-change rules (`protect.rs`), approved by the owner on 2026-11-11. The gate is looser only for files that are never executed. A Markdown file that a gate does read (none today) would be held by its path under `PROTECTED`, not by this scan.

**Tests.** `a_quoted_or_true_in_markdown_is_not_a_broadened_skip`: the quoted note in a stage note, and `cargo test || true` in `README.MD`, are not flagged. The same line in a `.sh` file or a workflow still is.

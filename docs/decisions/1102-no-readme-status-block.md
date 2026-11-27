# 1102: landing no longer writes README.md

Decided by the owner (task #1141): MASON stops changing `README.md` with every landing. 167 of the 169 commits to the file in the 7 days to 2026-11-25 were the regeneration commit, and the generated list had become most of the file.

This replaces the status-block parts of [0021](0021-numbers-readings.md): the "README status line", "Who writes the block" and the prompt's sentence about the block. The rest of 0021 (the reserved numbers and the landing check) stands. 0021 itself is left as written.

- **Removed.** `scripts/gen-status.sh`, `scripts/check-status.sh`, `Batch::regenerate_status`, `Batch::drop_status_commit`, `STATUS_SUBJECT`, `STATUS_DEADLINE` and their calls. A moved base is now folded onto the integration head as it is.
- **The prompt** (it goes to agents of every project) says: do not add an entry to README.md for your change; edit it only when your change makes something it says wrong. It no longer asks for a `Summary:` line. Older exit scripts that look for that line in their own note are unchanged.
- **Kept.** `scripts/gen-*.sh` stays protected in `protect.rs`. CI keeps `paths-ignore: ["README.md"]` and its test in `ci_gates.rs`.
- **README.md is not edited here.** A daemon on the old build keeps appending to the list until the generator is gone from the tree it assembles, so an edit there would conflict. A follow-up task removes the list.
- **Protected paths** (owner approval): `crates/landing/src/batch.rs` and `crates/daemon/src/landing.rs` (the removal), `scripts/gen-status.sh` and `scripts/check-status.sh` (deleted), `scripts/exit/s127.sh` (drops the `check-status.sh` call; the banner and social card checks stay), `scripts/exit/s43.sh` (the assertions on the generated list are replaced by assertions that landing leaves README.md unchanged and commits no regeneration), `.github/workflows/ci.yml` (a comment only). The removed check and assertions only guarded the generated list.

# 1116: s724 follows the doctor's workflow wording

## Context

`scripts/exit/s724.sh` greps for the line `mason doctor` prints when the remote-run workflow on a base branch is fine. The line in `crates/cli/src/doctor.rs` was reworded (it now names `workflow_dispatch` and pushes to `mason/run/**`) and the script was not, so it failed every time.

## Decision

- The script's expected string becomes the printed line, matched with `grep -F` in single quotes, so the backticks and `**` are literal.
- The code's wording stays; it describes what is checked now.
- `scripts/exit/**` is protected and this edits an existing gate (decision 0019), so the change waits for the owner's approval.

## Consequences

A later rewording of that line must update s724 in the same change.

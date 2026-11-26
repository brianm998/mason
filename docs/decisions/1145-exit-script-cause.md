<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# 1145: the cause of a red exit script is a failure line

## Context

`CI/exit-scripts (ubuntu-latest, 4)` was red twice on develop 93e5f2ac0 (run 45788017032),
with s22 and s777. The full job log was not available to this task: only the excerpt in #1200,
which has these lines and omits the "last 40 lines" blocks of both scripts:

```
  cause s22: == 1. a retry loop on a permanent error, planted in recorded history
  cause s777: FAILED: the local check did not build as a step of its own
FAILED scripts: scripts/exit/s22.sh scripts/exit/s777.sh
```

### s777

The failing line is the second one above, from the grep for
`build already passed for this tree; reused.*task=1` in `masond.log`. masond coloured its log
even when stderr was a file, so the log held escape codes between `task`, `=` and `1` and the
grep could not match. Commit 43f71f6f0 fixed that. It is not in 93e5f2ac0, so this red predates
the fix. Nothing more is changed for it.

### s22

The root cause of this run's failure is **still open**. The cause line is a heading: `cause_of`
took the first line matching `[Ee]rror`. Task #1194 records s22 timing out in stage 3 ("timed
out waiting for the analyst's proposal") in run 45748863128 on 53e3c2927. That message matched
none of the old patterns, which would give the same heading as cause, but that is an inference:
this run's log was not read. The fix belongs to #1194, so s22.sh is not changed here and no
wait is lengthened.

## Decision

- `cause_of` ignores lines starting `== `.
- It no longer matches `error` anywhere. It matches panics, `FAIL`, `not ok`,
  `timed out waiting`, `error:` or `error[Exxxx]:` at the start of a line or after a colon,
  `Error:` and `fatal:` at the start of a line, `SomethingError:` and `##[error]`.
- When nothing matches, the last line of output is the cause.

## Consequences

- A failure worded in another way is reported as `last output: ...` instead of whatever earlier
  line happened to mention an error. The full tail is still printed above the causes.
- No assertion in any existing exit script changes.
- `scripts/run-exit-scripts.sh` is a protected gate (`crates/daemon/src/protect.rs`), so the
  change needs the owner's approval.

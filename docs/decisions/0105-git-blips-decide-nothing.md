# 0105: A brief git failure decides nothing

A momentary git failure was read as a final answer in several places. A failed `git show` of `landing.toml` gave the defaults (`direct`, no window), which would push a `pr-each` or `host-queue` project straight to its base past its reviews. A failed `rev-parse` gave the built-in definition. A failed fetch failed the task.

**Changed.**
1. `prepare`: a fetch failure is logged and the step goes on from the last-known base when that ref exists (otherwise it fails, naming both). Timeouts and lock errors elsewhere in `prepare` are waited out (`WaitStarted`, backoff 1 s to 30 s, 15 minutes) and then fail naming what failed; nothing is charged to the task. Real errors stay errors.
2. `load_config` / `read_config`: missing file means defaults; a failed git call is `ConfigError::Unread`. The batch loop then leaves the queue as it is, retries every 2 s, and raises an alert after 5 minutes. **`crates/daemon/src/landing.rs` is a guard; this change only makes it stricter** (a read that failed can no longer relax the mode). It needs the owner's approval as a guard change.
3. `read_definition`: git failure is an error through `definition_error`; the current definition stays.
4. `numbers::base_paths` and `added_files` now return `Result`. Before, an empty result would let a number be handed out twice or an added numbered file pass the check. `ensure_numbers` errors; the agent step and landing's number check wait out environmental failures; a real error in the check is a failed verdict.

**Left, and why.**
- `numbers::adhoc_exit_room` (advisory, called when filing a task over HTTP): on a failed read it says there is room. The real reservation reads the base again and fails, so nothing is granted on it.
- `sweep_integration` (`unwrap_or_default()` on the integration branch list): an empty list only means fewer disposable branches are deleted this time; nothing is decided.
- `suites::changed_paths(...).unwrap_or_default()` in `submit_and_wait`: empty paths select suites by impact; left as is here because a selection based on empty paths is what #57's follow-ups should settle with the suite code (it may under-select tests). Flagged, not changed.
- `watch_definitions` treats a failed `rev-parse` as "changed" and reloads, which now reports the error and keeps the current definition each tick until git answers.

**Not verified:** nothing was compiled or run by me; see the stage note.

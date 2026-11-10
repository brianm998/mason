# 0078: Replay applies events in place; `world()` is a shared snapshot

Restart time grew with the square of the history because `reduce` cloned the whole world per event and `world()` cloned it per poll. Now:

- `reduce_in_place(&mut World, &Event) -> (effects, Touched)` is the transition; `reduce` stays as the pure wrapper (clone, then in place) so its callers and the golden-log tests are unchanged. Every refusal in `apply` happens before any change (the one arm that edited part-way, `BlockersChanged`, now restores the blockers it touched), and `now` is put back, so a refused event leaves the world as it was.
- `Store::world()` returns `Arc<World>`; the writer holds one `Arc` and appends with `Arc::make_mut`, so it copies only when a reader still holds a snapshot at that moment.
- **Why `Arc::make_mut` and not cheap-to-clone `Task`/`StepRun`:** it removes the copy instead of shrinking it (cheap clones would still walk every task and run on each append), and it needs no change to the types every caller reads. The cost is that an append while a reader holds a snapshot copies once (including a refused one); pollers hold theirs briefly, so this is occasional.
- **Rollback:** if the SQLite transaction fails after the world changed, the writer rebuilds the world by replay from the log (rare; a failure of that panics, and the writer-panic rule restarts the daemon).
- The projection write no longer diffs old against new world. `reduce_in_place` returns the rows it touched (the event's own task, plus every task `refresh` changed) and the writer rewrites exactly those. A missed row would show in `verify()`, which compares the projections to a replay.
- `World::refresh` mutates tasks in place and writes only those whose blockers or activity changed; nothing is removed and reinserted.
- `World` keeps an `open_runs` index maintained by `enter`/`leave`; `open_run` is a lookup and `check_invariants` proves the index equals a scan of the step runs.
- `verify()` opens its own short-lived read-only connection instead of holding the shared reader's mutex for the replay.
- `World::clone` is hand-written and counts, so tests can assert "no clones" (`mason_engine::world_clones`).

Files in the guard/trust lists are not touched.

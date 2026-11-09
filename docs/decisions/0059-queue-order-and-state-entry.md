# 0059: Queue order and state entry in the sidebar (s73)

- **The daemon sorts; the clients do not.** `section_of` is the one place. Conformance already compares each client's row order with the server's, so no client change is needed. Only comments in the proto and the generated Swift file changed.
- **Queued uses `queued_rank`, made `pub(crate)`,** so the panel and `next_admission` cannot disagree. Tasks whose activity is `Waiting(Slot)` come first, then the rest (held by blockers, paused, waiting on `not_before`), each in rank order. Which account has a free session is deliberately ignored, so rows do not jump as sessions free up.
- **`state_entered_at` is written in one place, `reduce()`.** It compares each task's state before and after the event, so a future transition cannot forget it. A second step in the same category, a retry that stays in a state, and ticks do not count. Filing sets it explicitly. Replay runs the same `reduce()`, so tasks already in the store get values with no backfill.
- **Not in the `tasks` projection table, nor `dump_world`.** The field lives in the replayed `World`; no projection test needed it.
- **`row()` / `state_since` is left alone.** It shows time since the last step start. Making it say time in state would change every row's display, which was not asked for. Recommended follow-up.
- **Needs you** orders by when the task entered the state; a task waiting on an approval without a state change is ordered by its earlier entry. Good enough, as the task says.
- **The Home attention list** still sorts oldest first by filing: it is not the left panel.
- **Protected files:** `crates/core/src/task.rs` (one field), `clients/features.toml` (description text only) and `scripts/exit/s73.sh` (new). No gate, guard, budget or verdict logic changed.
- **Numbers:** 73 and 0059 were handed out by MASON.

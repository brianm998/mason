# 0962: An owner retry or Land now forgets the cached red landing runs of that task

Stage s969. `Runner::cached_run` reuses a stored landing result for the same project, base, task
heads and filter, and `keep_run` stores every result except service errors. A red is always
reused. On 2026-11-22 task #841 had one real batch run (batch #607) whose test-cli build failed on
two symbols that exist on develop, in files the task never touched: a bad batch build, not the
task's code. Batches #612, #614 and #627 each reused that red. The owner retried the task from
land and pressed Land now, and nothing ran test-cli again; the task failed after three land rounds
without a new test. The only ways out were a new base, a new head, or deleting rows by hand.

Decision:

- `Store::forget_red_batch_runs(project, task)` deletes every red `batch_runs` row of the project
  whose task key includes the task, at any head. The id is matched whole in the
  `id@head,id@head` key (`841` does not match `84`, `8410` or `1841`). Greens stay.
- `http::forget_red_runs` calls it and logs one INFO line with the count. It is called when the
  owner asks for the task to land again:
  - `POST /tasks/{id}/retry` (the CLI's `mason task retry`, the monitor and the websocket command
    all come here), through `RetryAsk::forget_red`, before the `Retried` event so no batch can
    see the old red;
  - applying a diagnosis by hand (`diagnose::apply_args` with `auto` off, a retry verdict);
  - `POST /tasks/{id}/land-now`, and `POST /batches/{id}/land-now` for each member it flags.
  `force` does not: it lands without verification and reads no cached run.
- Retries the daemon makes by itself keep today's reuse: `autoretry`, a diagnosis applied with
  `auto`, the health agent's retry, a batch splitting and a parked task re-joining. That reuse is
  what stops a truly red task from running again and again.
- The `cached_run` doc says a red is reused until the owner retries or lands now.

Not done: no expiry of a red by age (prune still keeps two days), and no change to what is
cached.

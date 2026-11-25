# 1110: A remote run GitHub fails waits again at its place

Status: asked for by the owner on 2026-11-21 (task #822). It touches `crates/daemon/src/suites.rs`, `crates/daemon/src/landing.rs` and `crates/daemon/src/sched.rs` (protected files), so it waits for the owner's approval. No verdict, gate or budget changes: only how a run waits after GitHub fails to start it.

Decision:

1. After an environmental failure the run's runner units go back and the run asks for them again at its original rank and wait start. It holds none while it waits.
2. The first retry is asked for at once. From the second failure in a row, the next request is made only after `env_backoff` since the last one.
3. An agent's check and a landing check end after three environmental failures in a row (before: two tries). A run cut off at the runner limit keeps its one retry, so a hang cannot cost a third hour. `fallback = "local"` keeps two tries, then this Mac.
4. A landing check that used its three tries ends the batch without #772's further 30 minute wait. Where GitHub would not take the run at all (push or dispatch refused), #772's wait stays, since three quick refusals say little about how long an outage lasts.
5. The test step keeps retrying without limit, as before.

Not verified: see [the stage note](../stages/s1121.md).

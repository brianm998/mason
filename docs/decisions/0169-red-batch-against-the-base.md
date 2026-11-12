# 0169: A red batch is compared with the base branch before anyone is blamed

Star's batch #103 failed CI on a test that was already red on develop, and the task got only job names: no log, and the blame for a red it did not cause.

**Logs.** The adapter keeps the end of every failed job's log (`JobFailure.log`), judge on or off, and the landing puts about 60 lines of them in the batch notes, which become the task's feedback.

**Comparison.** For each failed job the landing finds the base branch's latest finished run of the same workflow, the job of the same name, and compares failing tests (read from pytest and cargo output). It reuses the #164 reading (`Source`), not a second one.
- Every failing test also fails on the base with the same text: nobody is blamed, the batch waits ("red on <base> too").
- Same tests, different text (`['.mason', 'scripts']` against `['.mason']`): nobody is blamed, the batch waits and a `landing` alert names both texts for the owner.
- Anything else, including anything unreadable, blames the batch as before. The comparison only ever removes blame when it can show the failure is already on the base; a doubt leaves today's behaviour.

**Waiting** reuses the service-error path: the batch ends with the reason, nobody is parked. Where #164's freeze is configured it freezes landing too. Without it the batch is tried again on the next landing round and holds again while the base is red.

**Trust.** `crates/daemon/src/landing.rs` is protected (guards: the red ladder); the change needs the owner's approval. It adds a way for a red to not be blamed, never a way for a red to land.

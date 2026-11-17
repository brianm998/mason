# 0501: a killed develop run is "unknown", not "green"

**Decision.** When a red test step compares with develop, it runs only the failing tests through the suite's `rerun` template. A run that did not finish or read no names is retried once. If it is still unknown, the round is not counted against the task: the suite ends `Excused` (closing `unverified`) with a note saying so.

**Why.** On 2026-11-16 a killed full-suite run on develop made #452 take the blame for a test that was red on develop. A missing answer must not be read as "not red on develop".

**Cost.** A change that really broke a test can pass a round if develop cannot be asked twice; the closing verdict records it as unverified, and the next suite run still checks the tests.

**Trust.** `crates/daemon/src/suites.rs` is in `PROTECTED` (verdict logic); this waits for the owner's approval.

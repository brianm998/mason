# 0174: reuse a command's green by exact tree and command; refuse a known red

**Context.** The suites path skips a suite whose exact tree passed; the single-command path did not, so the same tree was tested by `run_check`, then again by the test step, and a red tree could be resubmitted unchanged.

**Decision.**
- Key: the content tree (from a temporary index over the working files) plus a hash of the resolved build and test command text. Exact only.
- Green is recorded by `run_check`, the command test step and landing's re-test; the test step and `run_check` reuse it. Only a whole-command green is recorded, not a filtered rerun or a base-excused one.
- Red is recorded by the test step only, when it is red by exit status on every run and the failing tests were named. `submit_change` refuses a head whose tree has such a red; a later green for the same key (for example a `run_check` that passes) replaces it. Environmental reds (timeouts, signals, unnamed failures) are never recorded.
- A separate table `command_runs`, not `evidence`, because it needs a red state, the source, and the names.

**Trust.** Close to verdict logic, but `suites.rs` and `crates/testing/**` are untouched. The owner should review that reuse cannot turn a red into a green: a green is only returned for an identical tree and identical command that itself passed.

**Consequences.** A flaky test that passed once on a tree stays "green" for that tree. The test step skipping the run also skips the flake recording for it.

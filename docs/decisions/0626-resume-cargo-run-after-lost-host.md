# 0626: after a lost host, a cargo run goes on from the executables that finished

Status: proposed (s633). Amends the rule in `hostloss.rs` that only the fresh host's run is judged.

Context: a lost host sent a 2,095-test run back to zero, three times for one test round in two days.

Decision: for a whole-workspace cargo suite, the lost run's log says which executables ended with a `test result:` line. The fresh host runs the other targets (rendered like `{tests_for_change}`); the verdict is judged on the lost run's finished blocks followed by the fresh output. A failed finished block keeps the run red, `min_tests` counts both, the note says what was reused. A mid-run executable, an unreadable log or a command the plan cannot place runs everything again. A second loss in a row fails the step.

Consequence: the evidence for a green is built from two hosts' runs of one tree. A run that was resumed records no run time.

Trust: `crates/daemon/src/suites.rs` and `crates/testing/**` are protected; owner approval needed.

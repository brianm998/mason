# 0469: a green that skipped quarantined tests pays a debt, with the tests named

Status: accepted (waits for the owner: `suites.rs` is a protected verdict file)

While any test is quarantined no whole-suite green is recorded, so no debt could be paid and idle runs repeated. Chosen: record such a green apart in `suite_skip_green` (base, suite, executed, skipped tests); `suite_green` stays whole-suite only. A debt is paid from it when its landed commit is an ancestor of that base and its own diff added or removed no line naming a skipped test; the skipped tests go in `how`. Otherwise it stays open until a run that executes the test. `run_idle_debts` runs each (project, scope, tip) at most once per pass and settles all debts after each run. Quarantine expiry (s416, lift_landed) is untouched.

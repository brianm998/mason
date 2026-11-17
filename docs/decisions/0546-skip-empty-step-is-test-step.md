# 0546: An empty step in a skip means the task's test step

Status: accepted (s553).

Context: the monitor's Skip testing for all sends each row's `step`, which can be empty for a task waiting in `base-red`. The daemon refused it and the Testing page showed neither that nor a refusal for a read-only monitor.

Decision: `suites::skip` reads `step:` with no name as the project's test step; the clients send the row's step as it is. Every client says what a Skip testing click did, and disables the button with a reason when it cannot act. The verdict effects (debts, `Gate::Skipped`) are unchanged.

Consequences: the CLI's `--step ""` does the same. A project with no test step refuses with "this project has no test step".

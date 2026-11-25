# 1000: only the Test step can be skipped

Decision: option (a). `suites::skip` refuses a `step:` scope whose step is not a Test step, after
the landing rename (`landing_scope`) has been applied, so a skip sent for a task's landing row still
becomes the Test step's. The error names what can be skipped and points at `mason task force` and
`mason task land-now`. Reason: only Test-step and landing code read step skips; honouring a skip of
review (b) would change verdict logic. Trust: `suites.rs` is in PROTECTED and needs owner approval.

`mason debt list` says "no effect" under an open `step:` debt whose step is not a Test step
(recorded before this change); the debt is not closed or altered. The monitor needs no change:
its only skip actions are "Skip testing", offered for tasks in testing or landing, and "all
tests"; it has no menu for choosing another step.

Not verified: tests and the exit script were not run locally (they run on GitHub via run_check);
the monitor was not exercised.

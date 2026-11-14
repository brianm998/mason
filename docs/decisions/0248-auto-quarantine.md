# 0248: quarantine repeat flakes automatically, one task each

Stage s253. Status: accepted by the owner.

## Context
Flakes were recorded (19 in a week) but quarantine was manual and its list was empty. One sched test flaked in ten tasks before anyone fixed it.

## Decision
- When a flaky record is made, a test of a suite that has flaky records from three or more distinct tasks within seven days, that is not quarantined, is quarantined for 14 days: owner `mason`, reason `auto: flaky in N tasks since <date>` (then `; task #K`, the task that tracks it).
- One task, "flaky: <suite> <test>", normal priority, is filed with it, unless an open task has that title.
- The quarantine is lifted when that task lands, and otherwise expires after 14 days. A quarantined test still runs and is reported; it only stops deciding the step.
- Only real test names are quarantined: a name must have been seen passing, in a green run of the suite or in a base run's list of names that did not fail. A parser that takes a log line for a test name never sees it pass, so it cannot be quarantined.

## Not done
The command path (a project without suites) records no passing names, so its tests are not auto-quarantined. Three flaky records from one task count once. A test that stops flaking for 14 days comes back by expiry, not by proof.

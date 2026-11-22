# 0927: a narrowed suite has no expected total

Decision: when a suite is narrowed or not run, its sub-progress total is cleared (unknown) rather than the whole suite's last green count.
Why: only whole runs record `suite_green`, so the stored figure is always the whole suite's size and misleads. Counting the selection from `test_times` is possible later.

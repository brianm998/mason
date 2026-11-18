# 0591: a failing Compose test must be named, and tests wait on signals

The red CI run did not name a failing test, and s13.sh hid Gradle's output and status, so raising deadlines (s572) was a guess. s13 step 1 now keeps the status and prints failing testcases. Tests that need a connection thread wait on a condition signalled by the fakes, with no deadline and no new timing assertion; a hang is reported by the job's own limit. No assertion was removed or weakened.

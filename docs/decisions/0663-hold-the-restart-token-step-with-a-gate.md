# 0663: hold the restart token step with a gate

Decision: the restart test's step waits for a release file rather than a fixed sleep.

A sleep long enough for a loaded machine only moves the failure; a gate removes the race, because the test, not the clock, decides when the step may end. `gated_develop` already does this for the first test in the file.

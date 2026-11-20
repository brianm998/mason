# 0684: lint runs before the batch suites, and a lint red is blamed by file

Decision: the landing lint tier runs before the batch suites in every verification and split run; a red stops there. Its output names files (`Diff in`, `-->`); every task whose footprint touches one is parked with the lint output, the others stay. With no touching task the existing ladder runs. A red whose only failing tests are ci_gates fmt tests is blamed the same way. A `fmt` suite (no build, no slot) in the test step catches unformatted changes per task, independent of a `cargo-workspace` skip.

Why: lint takes seconds and names the file; the whole suite takes 15-25 min and a bisect repeats it per step (batch #421, 148 min).

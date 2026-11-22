# 0770: a remote run_check builds and tests in one run

Status: accepted (#771). The `result.json` change in the protected workflow waits for the owner's approval.

A GitHub runner starts from nothing, so a separate build run does not warm the test run: both compiled the same ~200 crates and each cost about 55 s of plan and pickup. Locally the split stays (the build holds `compile`, the tests `test-suite`, decision 0130), because the target directory carries over.

Decision: when a check goes remote, send the build in `RunSpec.build` of the test run. The runner stops at a failed build; `result.json` says `phase` so the daemon can word the failure and record the tree's build result. A runner that does not say records nothing about the build.

Rejected: joining build and tests with `&&` (two leading `cd`s break); inferring the phase from `run_s == 0` (a test that fails in under a second looks the same).

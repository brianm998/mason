# 0589: monitor-swift step 1 checks swift test's own exit status

Step 1 of s12.sh piped `swift test` through `grep | head -1`; under `pipefail` the early-closed pipe made the step fail though every test passed. It now writes the output to a file, fails loudly (last 30 lines, exit 1) when `swift test` exits non-zero, and prints the last summary line. Inferred from the CI log, not reproduced locally. The change touches a protected gate and needs owner approval.

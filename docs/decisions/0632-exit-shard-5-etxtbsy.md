<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# 0632: retry the open_session spawn only when the fake never ran

Status: accepted (s639).

Context: exit shard 5 on ubuntu-latest failed s193, s219, s320 and s486. For s320 the tests write and exec a fake binary in parallel threads; a spawn racing another thread's fork can fail with ETXTBSY, which run_session reports as 127. This is inferred from code, not reproduced.

Decision: the test helper retries (up to 50 x 20 ms) only when the status is 127 and `<provider>.cwd` was not written, so a real failure of a fake that ran still fails at once. Assertions and run_session are unchanged.

Consequence: s193, s219 and s486 are not fixed and remain open until their failing output is read from the CI log. Follow-up for the owner: s193.sh's `cargo tree | grep -q` under pipefail can misreport when grep exits early; it is a protected file.

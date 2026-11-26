<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# 1151: retry the stub-host spawn race in the tests, not in the launcher

Status: accepted (s1161).

Context: `a_launch_is_listed_while_it_waits_and_gone_after_success` timed out on ubuntu: the launch was never listed, and its error stayed in the unjoined thread. The suspects before listing are the socket path check, creating the directory, opening host.err and the spawn. The path under the runner's temp dir is about 35 characters (assuming the default `TMPDIR`), far under the limit of 100, and the other tests use the same `root()`. The test writes a stub script and executes it while sibling tests fork, so the spawn can fail with ETXTBSY on Linux. The `launch` helper already retries that for the freezing stubs, but this test called `spawn_host` directly. This is inferred from the code, not reproduced.

Decision: the listing tests launch through `launch`. `launcher.rs` and `check_sock_path` are unchanged: the race comes from a fixture that has just written its own executable. If the cause is something else, the test now fails with the real error.

Alternative: retry ETXTBSY inside `spawn_host_within`. Not chosen without evidence that a daemon ever meets it.

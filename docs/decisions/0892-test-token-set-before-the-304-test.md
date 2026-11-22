# 0892: The 304 test sets its own token

`other_workflows_are_counted_and_a_304_changes_nothing_and_asks_for_nothing_more` read `MASON_TEST_REMOTE_TOKEN` without setting it. Only other tests set it, so the test passed or failed by test order. It now calls `token()` first, as the other remote tests do. No production code changed.

**Not verified:** see the stage note.

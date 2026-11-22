# 0876: the active-models test needs no further change after 0831

A task that is admitted but waits for its first llm session is still in an explore or develop-queue state with no live develop record, so `views.rs` `agent_state` does not list it (only a Developing step with a live record is listed as `waiting_slot`). The test's empty-list expectations stay right once the other session's lease is opened first (0831, commit 5eac1d1cf). The red recorded at b9d4aa3ef5 predates that fix; this task records that and changes no test.

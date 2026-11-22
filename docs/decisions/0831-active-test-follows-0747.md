# 0831: the active-models test follows 0747

The active listing follows decision 0747 (#763): a started agent step waiting for a check or its session is listed. The test `active_models_lists_only_tasks_that_hold_a_session` raised the llm limit before opening the other session's lease, so an admitted task took the session first and stayed listed. The test now opens the lease first. The view is unchanged.

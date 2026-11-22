# 0876: the active-models test follows 0747 for waiting-slot tasks

Admitted tasks whose agent step waits for an llm session are listed as `waiting_slot` with resource `llm:claude`; `sessions_used` counts only sessions held. The test `active_models_lists_only_tasks_that_hold_a_session` keeps its name (pinned by s129) and expects that. See 0831 and s838.

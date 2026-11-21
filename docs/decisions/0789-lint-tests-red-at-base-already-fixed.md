# 0789: three landing lint tests red at a61fedd09e were already fixed on develop

The base a61fedd09e predates b92093ab ("Run the landing lint tier on this Mac by default
(#811)", decision 0784). The debt367 journal shows the three tests timing out there: lint never
reached GitHub, the batch never landed, the task was never sent back. The fix and the tests are
both on develop; the tests are unchanged by this task. This records the finding and changes no
behaviour.

Tests: `the_lint_tier_runs_here_while_the_switch_says_github`,
`a_lint_tier_set_to_github_goes_there_and_holds_no_local_compile_slot_while_queued`,
`a_red_lint_run_here_parks_the_task_with_file_and_line`.

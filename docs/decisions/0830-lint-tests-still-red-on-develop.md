# 0830: the landing lint tests were still red on develop; decision 0789 was wrong

Status: supersedes 0789.

0789 said the three landing lint tests were already fixed on develop. Two were not.

- `a_red_lint_run_here_parks_the_task_with_file_and_line` asserts the sent-back reason holds
  `x/bad.rs:1`. The reason (`park`) only said `red in batch #N: lint`; the file and line reached
  the feedback alone. The reason now adds ` (at x/bad.rs:1)` when every failed name is a lint-tier
  suite, from the places stored when the lint run went red.
- `a_lint_tier_set_to_github_goes_there_and_holds_no_local_compile_slot_while_queued` never
  reached the fake GitHub. The landing lint named the batch's first member task as the owner of
  the check; the passing floor check names none. The lint now names none too, so the suite's own
  placement and the global switch decide. This root cause was read from the code, not reproduced
  locally; develop's CI is the check.
- `the_lint_tier_runs_here_while_the_switch_says_github` was green and stays so.

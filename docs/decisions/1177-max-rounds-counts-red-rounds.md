# 1177: max_rounds counts red rounds

Replaces s69's "the step's runs since the task's last restart" and its "approval steps still count their attempts".

A step's round is 1 (the current run) plus its earlier runs that ended `SentBack` since the last
`Failed`/`Superseded`/`Canceled` run, leaving out conflict runs. Green runs neither count nor reset.
One helper (`red_runs_before`) serves `Ctx::round`, `test_rounds_done` and `conclude_approval`, which
passes the same round to `after` and `repin_for_send_back`. Engine attempt numbers are unchanged.
Last-round records are keyed by run id so a reused round number never reads an old file.

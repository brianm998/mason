# 1175: No rerun of cancelled or outdated full CI runs

Status: accepted

A run whose own conclusion is `cancelled` was cancelled by someone; its cancelled jobs are not failures. It is recorded cut off and nothing is rerun or filed. When develop's head differs from the run's commit, the failed jobs are not rerun and no tickets are filed; the old run is recorded cut off, and the schedule dispatches on the current head next. A manual `schedule run` while a rerun of an outdated commit is followed cancels that rerun and dispatches; a rerun of the head is respected. The simple rule (develop moved) was chosen over a path comparison.

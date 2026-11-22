# 0870: the analyst's result ends its task

Status: accepted (s877)

The analyst task's only product is the findings file. Running it through test, review and land made reviewers reject it for the missing file and let rewrites file duplicates. So a read result closes the task (`ClosedNoChange`), ingest runs once per task, and the file's copy lives in the task's log directory. Not verified end to end in a live pipeline.

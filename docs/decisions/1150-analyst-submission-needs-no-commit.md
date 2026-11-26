# 1150: The analyst's submission needs no commit

## Context

The analyst's task tells the agent to write `analyst-result.json`, not to commit it, and to finish. The develop step's `submit_change` check refused that: the tree has an uncommitted file and the branch has no commits. Since cb040fac3 the findings are read only after an accepted submission, so the task failed and nothing was filed. This is why `scripts/exit/s22.sh` (and s1035, which runs it) timed out.

## Decision

For the analyst's task only, known by its schedule row (`introspect::is_analyst_task`) and asked at submission time:

- an untracked `analyst-result.json` does not make the tree dirty;
- a branch with no commits beyond the base is accepted.

Every other check stays, for every task. The analyst's branch is never merged (the task ends with `ClosedNoChange`), so these two proofs protected nothing there. A missing or unreadable result still sends the task back to develop.

This changes which submissions a gate refuses, so it needs the owner's approval.

## Alternatives

- Give the analyst's develop step no validator. Simpler, but it also drops the head, other-leftover, marker and self-guard checks, which nothing requires.
- Read the findings whatever the step's end (as before cb040fac3). The task would still fail and ask for attention each time.
- Tell the analyst to commit the file. It would put the result in a branch for no purpose, and the file would no longer be removed cleanly from the tree.

## Also

`ingest_analyst` now logs at WARN, with task, project and worktree, when the analyst's task left no result, and when a task carrying the analyst's title is passed over because the project's analyst schedule does not name it.

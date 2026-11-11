# 0117: The dropped-line audit

Task #108. When a task's merge with the base resolved a conflict, every line the base added
since the merge base must still be in the result. Choices the design left open:

**Where it runs.** Two places. (1) `submit_and_wait`, next to `check_numbers`: each merge commit
on the task branch's first-parent chain whose second parent is an ancestor of the base is
audited against the branch head. (2) `Batch::assemble`: right after a resolver commit, the task
is audited against the base tip the batch is built on, with the integration head as the result.
Clean merges in a batch are not audited.

**Merge base for merges in the task's branch.** For the merge with parents `p1` (the task's side)
and `B` (a base commit): `merge-base(p1, B)`; the base's added lines are `merge-base..B`, the
task's removed lines are `merge-base..p1`. The result is the branch head, not the merge commit,
so a later commit that restores the line satisfies the audit. Lines the base itself removed
after `B` (`B..base tip`) are not the task's to keep: a task that merged an old and a newer base
commit would otherwise be blamed for the base's own deletion.

**SentBack, not Conflict.** The agent's resolution is at fault, so it is a red round
(`Verdict::SentBack`) with the reason `conflict resolution dropped N landed line(s) in: files`.
`Conflict` is for "the merge itself failed". In a batch the task's merge is undone
(`reset --hard` to the head before it), so the rest of the batch still merges and lands.

**The way out: `removes` in `submit_change`.** A new optional array of `{file, line, reason}` in
the step contract (`crates/adapter-api/src/contract.rs`, schema and validation: all three
fields are required and non-empty). An accepted Developing submission is written to
`change.json` in the task directory (as `plan.json` is); landing reads it. Lines are compared
trimmed, per file. Task detail shows the list after the route text. The task-detail protocol
message has no field for it and adding one would change the generated client code, so it is
appended to `route` for now.

**Token rules** (ported from starpipe, not the Python): a token is an identifier (letter or
underscore, then letters, digits, underscores) or a digit run. A line of at least 3 tokens is
kept when all its tokens appear within a window of 3 adjacent result lines; shorter lines must
match exactly. Blank lines, lines with no word character and `source-hash:` lines are ignored.
Diffs are read with `-U0`; only lines inside hunks count, so file headers never look like content.

**Limits.** Only merges of base commits are audited. In a batch, the integration head holds
earlier tasks too, so a base line deliberately removed by an earlier batch task is flagged
unless listed. File contents are read as lossy UTF-8. Lines are matched, not moved: a base line
moved to another file counts as dropped.

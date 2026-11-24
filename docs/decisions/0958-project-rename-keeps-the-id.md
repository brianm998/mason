# 0958: A project rename is an event plus a store rewrite, keyed by the unchanged id

Stage s965. A project's name was the stem of its registration file and was recorded by `ProjectRegistered`, in about 30 tables with a text `project` column, and in keys and paths. Moving the file only produced a new project with a new id and orphaned the history.

Decision:

- The name changes by a new event, `ProjectRenamed { project, name }`, not by editing the log (the append-only triggers stay). The id stays; the reducer refuses an unknown id or a name in use.
- The text columns and keys are not events (like the other tables that describe the pipeline), so `Store::rename_project` rewrites them in one transaction, finding the tables from the schema so later tables are covered.
- The daemon refuses while any task is neither landed nor canceled or a batch is running, because those hold branches, worktrees and step hosts named after the project. The file is moved first and put back if the event cannot be recorded; once the event is in the log the rename stands and a restart replays it.
- Old branches and worktrees: landing worktrees are removed (they are rebuilt per batch); branches carrying the old name are left and listed.

Alternatives not taken: re-registering under a new id and copying history (loses the id tasks and monitors know); editing `project_registered` (forbidden by the triggers).

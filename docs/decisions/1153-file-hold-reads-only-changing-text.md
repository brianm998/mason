# 1153: the file hold reads only the part of a body that says what changes

## Context

With no plan, the file hold (s398) takes file names from the task body. Every MASON body ends with the standard footer, which names `docs/self-hosting.md` and three scripts, so a task in flight on one of them held every queued task for up to `hold_back_secs`. Trust sections likewise name `crates/daemon/src/protect.rs` only to say what is protected.

## Decision

Cut the footer and the Trust section from the body before reading file names. Both markers are matched at the start of a line. The Trust section ends at the next line that starts with a `**…**` heading. `paths_in_body` is unchanged and still reads plan files.

`hold_back_ignore` was not used for this: it also turns the hold off for tasks that really change those files.

## Consequences

- A file named only in a Trust section or after the footer never holds a task. Every file a task changes is expected in Cause or Change.
- A line inside Trust that starts with bold text ends the section, and the rest is read as before.
- Only which files are compared changes; no verdict, gate or limit does. `file_hold.rs` is not a protected path.

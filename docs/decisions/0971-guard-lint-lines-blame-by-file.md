# 0971: guard lint lines blame by file, filtered by the batch's diff

Status: accepted (owner asked for the task 2026-11-22).

A lint red from a repository guard prints `<path>: <message>`. `lint_files` accepts such a line only when the path is in the batch's union diff, so prose with a colon can never blame a task. Guard scripts are unchanged and no check is weakened. `landing.rs` is in `PROTECTED` and needs the owner's approval.

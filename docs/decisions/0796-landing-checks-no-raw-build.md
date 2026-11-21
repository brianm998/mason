# 0796: landing checks carry no raw build template

Status: accepted (verdict-logic repair of #815, owner asked 2026-11-21).

A landing check's `cmd` is either the expanded build (floor) or a command that needs no separate build. The place suite therefore has `build: None`, and `remote_check` refuses any build still holding `{`. Rejected: expanding the template inside `remote_check`, which lacks the project's test command.

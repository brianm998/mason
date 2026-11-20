# 0678: map the paths that ran every suite whole, and list what is still unmapped

An unmapped path selects every suite whole, by design, and 11% of recent tasks hit one. We map the
paths by checking what reads each file (a cargo test reading it means cargo-workspace; nothing
reading it means no suite), and keep `.mason/mason.toml` unmapped so a definition change tests
everything.

To keep new gaps from going unseen, `mason definition check` lists tracked files that no glob
names, grouped by top-level directory, and the daemon warns the same at definition load. The
test notes now name the path that caused a whole run.

Alternatives: a catch-all `**` = [] was rejected (it would hide every new path); a catch-all
`cargo-workspace` would keep today's cost.

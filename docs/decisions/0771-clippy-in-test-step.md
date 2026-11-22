# 0771: Clippy in the test step

Status: proposed (task #773, waits for owner approval: `.mason/mason.toml` is a gate)

Decision: add `[suite.clippy]` to the test step, narrowed by `narrow_packages`, selected only for
`crates/**`. A small wrapper script turns cargo's output into one pass/fail line. The landing
`lint` tier stays, for semantic conflicts between batch members.

Alternatives: parse clippy's own output (many lines, no stable count); add clippy to the cargo
suite (a skip of that suite would skip lints too, and it needs `compile` held for longer).

# 0569: Exit scripts use the `it` test binary

Daemon integration tests are consolidated into one target, `it`. Exit scripts must run
`cargo test --test it <module>::<test>`; per-file targets such as `limits` or `stop` do not exist.

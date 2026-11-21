# 0729: shard the whole suite 8 ways on GitHub

Status: accepted (owner asked for it on 2026-11-20).

The whole `cargo test --workspace` run exceeds a GitHub runner's 40-minute limit (run 44730322603 was about 90% done at the cut). #701 (decision 0722) built sharding; the keys were commented out until the daemon reading them ran. It does (ca5406fb), so `shards = 8` and `shard_command` are enabled. Verdict logic is unchanged: the parser and `min_tests` judge the merged output.

# 0645: Resume keeps doc tests apart from target flags

`cargo test --doc` cannot be combined with `--test` or `--bins`, so `affected::render` emits the doc run as its own entry. The resume test expects that split form (`-p a --test two`, `-p a --doc`) rather than a combined command. The test was stale, not the plan.

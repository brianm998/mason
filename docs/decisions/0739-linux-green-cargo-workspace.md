# 0739: make the cargo-workspace suite green on ubuntu-latest

Status: accepted

Nine tests were red in the first whole-suite remote run. Each is fixed in the test unless it found a real defect: `mason remote <unknown>` looked for a daemon before it rejected the word, so it now rejects first (that is a real, small CLI bug and not Linux-specific). No test is gated by `cfg(target_os)`; none tests something that exists only on macOS. One edit is to `.mason/mason.toml` (`[impact]` maps `scripts/test-shard.py` to `cargo-workspace`); `.mason/**` is in `PROTECTED`, so it waits for the owner's approval. No other protected file is touched.

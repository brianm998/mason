# 0739: make the cargo-workspace suite green on ubuntu-latest

Status: accepted

Nine tests were red in the first whole-suite remote run. Each is fixed in the test unless it found a real defect: `mason remote <unknown>` looked for a daemon before it rejected the word, so it now rejects first (a real, small CLI bug and not Linux-specific). No test is gated by `cfg(target_os)`; none tests something that exists only on macOS. No protected file is touched (`.mason/mason.toml` already maps `scripts/test-shard.py` on `origin/develop`). The draining-update test is not fixed; see the stage note.

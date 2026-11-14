# 0312: macOS exit scripts: nothing to fix, nightly alone is a thin guard

Decision: no change to any script or code. The 13 macOS failures in nightly 43585753590 do not reproduce on current develop on this Mac; they also failed on Ubuntu in that run, when develop was red from batches b164-b172 landing without a build.

**Ran:** s05, s20, s25, s40, s50, s131, s193, s209, s219, s220, s226, s266, s267 (all pass) and s09 (fails, ordering check at s09.sh:215, same on Ubuntu, #333's).

**Not verified:** the next nightly (03:17 UTC) on GitHub's macOS runner, which differs from this Mac (it may lack `timeout`, which s226 uses without a fallback). s08, s129, s165, s166, s236 (#333). No behaviour difference on macOS for them was observed because they were not run.

**Is the nightly a sufficient guard?** For a failure that is macOS-only, it delays discovery by up to a day, and every push run shows the macOS jobs skipped. Recommendation for the owner (ci.yml is protected and untouched): run `monitor-compose-macos` and the exit shards on push only when a path filter matches `scripts/**`, `crates/**` or `Cargo.*`, or run one macOS shard per push as a smoke check. The owner decides.

**Trust:** no protected file changed.

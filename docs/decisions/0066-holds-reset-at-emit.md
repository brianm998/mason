# 0066: Test resets are relative to emission

Test fixtures that express a usage-limit reset must be relative to the moment the limit is emitted (`resets_in` in the fake agent's `rate_limit` action), not an absolute time computed at test start. The daemon drops resets already past when it observes them, so an absolute fixture fails on a slow machine. The daemon's stale-reset rule is unchanged.

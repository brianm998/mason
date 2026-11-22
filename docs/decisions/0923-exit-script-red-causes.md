# 0923: Exit-script red causes

Two causes were established from code: s799 grepped placement text landing.rs no longer has; the suites_schedule tests ran against a tree whose evidence the landing run had written, so the suite was reused and ran 0 times. Tests now clear `evidence`; no assertion or time range was loosened (decision 0755) and suites.rs is unchanged.

The CI log of run 45005421555 was not read, so the first panicked assertion per script is not recorded. Unexplained: s449, s50, s117, s193, s209, s219, s521, s691, s758, s77, s782, s798, s800, s808, s819, shards 4 and 5. Settle with: `gh run view 45005421555 --log-failed | grep -nE 'panicked at|FAILED:|== scripts|FAILED scripts'`.

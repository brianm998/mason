# 0716: runner pools for GitHub, and remote runs excused from the machine's gates

Status: proposed (owner approval needed: `crates/daemon/src/sched.rs` and the scheduler's plan are budgets)

GitHub's free plan runs 20 jobs at once per account, at most 5 of them macOS, shared by MASON's
CI, star's pr-gate and the iOS workflows. A remote run (#697) that took the suite's `compile` and
`test-suite` held this Mac's slots it does not use, and one that pushed as soon as it was ready
would queue in GitHub, invisible to MASON, ahead of star's macOS gate.

**Decision.**

1. Two pools exist by default: `gha = 16` and `gha-macos = 4`, changeable in `resources.toml` `[pools]` and by `mason resources limit`. That leaves four jobs and one macOS job for workflows MASON does not start. A macOS run holds one unit of each, as GitHub counts; a run sharded N ways (#701) holds N.
2. A remote run holds only those units: its lease request has `compile`, `test-suite`, `test-small` and `exclusive` replaced by them (`remote::pool_needs`, applied in the one function that requests a step's leases). The CPU, thermal, stall, syspolicyd and disk gates apply only to requests that touch machine-class resources or `compile`, so they never held a pool-only request; the memory floor held every request and now excuses a request for these pools alone.
3. The units are held from the lease until the run completes or is cancelled. A hold file written beside the run's record lets a restarted daemon count them until the record says the run ended; the resumed step takes them over.
4. A wait names the pool and the holders: "waiting for a GitHub macOS runner: 4 of 4 in use (#701, #705)", ranked by the usual rule.

**Not chosen.** Counting the account's other jobs (`[remote] watch_account`) was optional and is left for a later task. Holding the units inside `remote::execute` instead of the step lease would have needed a task and a rank there; the step lease already has both, and its wait text and cancellation.

**Risk.** A project whose remote run falls back to local (`fallback = "local"`) runs that command holding pool units and no `compile` slot; landing and debt payments still use the machine's slots until #702.

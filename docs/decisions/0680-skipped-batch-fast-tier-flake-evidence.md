# 0680: skipped-batch fast-tier flake: evidence first

Ruled out by reading (see 0657, 0669): the 900 s wait floor (tests/support/mod.rs), the idle debt pass and settle_debts (landed tasks only, suites.rs), git races, and the Members.stop path that skips the build floor (it reads debts the test writes before admission).

Open: (a) the verdict falls back to the suite name `alpha` when no test name is read (landing.rs, fast_failures); (b) the fast run is timed out or its output is cut (hostedcheck.rs collect).

Decision: change no logic and no assertion. The test's failure message now carries both reasons and the fast tier's log tails. A real fix follows the first red.

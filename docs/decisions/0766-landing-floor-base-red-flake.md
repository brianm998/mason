# 0766: red-base landing-floor flake (#789)

Status: unconfirmed; diagnostics added.

Only `base_builds` after `build_floor` failing yields the note "does not build". Any other outcome
of the batch makes the test's wait time out. No failing run was reproduced, so the cause is not
named and no `src` change was made (as with decision 0657).

Ruled out by reading: the 900 s floor (too slow to be a timeout alone), the evidence skip in
`build_suites` (no suite runs on the base here), the base-tree attach in `hostedcheck::run_with`
(run ids differ), `hold` cancelling via `alive()` (the task keeps its open run until its verdict).

Still open: `hostedcheck::collect` returning HostLost or "cannot collect" under load, `hold`
kernel-lock waits, a batch yield.

The test now panics with every batch row (phase, note) and the alert list on timeout. The next red
run's note picks the fix: a service error means retry a build with no result; a yield means the
wait must accept the later `failed` row.

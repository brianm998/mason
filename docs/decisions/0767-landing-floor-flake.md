# 0767: do not guess a fix for the landing-floor flake

Decision: no source change for the flake of
`landing_floor::a_batch_that_does_not_build_is_not_landed_and_the_task_that_broke_it_goes_back`.
The cause is not found, and tests run only on GitHub, so it could not be reproduced here. The test
now reports what the batch was doing when a wait times out or an assertion fails. Rejected:
changing waits or adding timing assertions.

Evidence: GitHub run 44939331878 (2026-11-21, only `landing_floor` selected) timed out all three
build-floor tests at their first wait: this test after 906 s ("the task that builds to land"),
`two_changes_...` after 900 s, and `a_base_...` after 600 s waiting for a world slot plus 900 s.
In the same run, `a_tree_that_already_passed_...` (floor skipped on evidence) and the conflict
marker test passed in under 10 s, and the batch-tier test passed 2 s after its 600 s slot wait.
So the failure is a hang with no verdict, not a wrong outcome. An earlier note here said the
opposite, and it was wrong. The quarantine table shows the three tests flaked together in tasks
#728, #729 and #740. That points at what they share, a build-floor build that runs to the end
(`landing.rs` `build_suites`, `hold` then `hostedcheck::run`), and not at this test's asserts.

Ruled out by reading: the build itself hanging (the scratch `test_timeout_s` is 60, so the
deadline thread stops it); machine gates (scratch daemons inject a CPU reading and start no
stall, syspolicyd or thermal sampler, and no memory floor is set); a compile slot shared across
tests (each scratch daemon has its own broker); remote placement (the scratch project has no
`[remote_runs]`); landed-then-on_main ordering.

Still open: `hold` never being granted the batch's `compile` unit, and `hostedcheck::collect`
waiting on a host that never reports its exit. The test now prints the batch rows, both tasks'
sent-back reasons, and every file under the data dir whose name has `-floor-` (the floor's logs
and host records). The next red run shows which of the two it is.

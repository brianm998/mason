# 0767: do not guess a fix for the landing-floor flake

Decision: the flake of `landing_floor::a_batch_that_does_not_build_is_not_landed_and_the_task_that_broke_it_goes_back` has no cause found by reading, and no reproduction was possible in this session (tests run only on GitHub). The test's assertions now print both tasks' sent-back reasons, the batch rows and the fast-tier logs. Rejected: changing waits or adding timing assertions (waits are already floored at 900 s by `wait_poll`, so a flake is a wrong outcome, not a slow one).

Ruled out by reading: landed-then-on_main ordering (promote precedes the Landed verdict), shared compile slots (per-scratch broker), hostedcheck key collisions (per-scratch hosts dir). Open: the build floor's selection/evidence short-circuit and the red ladder's reason text, which depends on which suites the batch reports red. The next failure's message should say which.

# 0226: wait out a timed-out fetch when there is no last-known base

A fetch timeout is environmental. With a last-known base the step goes on from it (unchanged); without one it now retries the fetch under `wait_out_environment` rather than failing the task. Non-timeout fetch errors still fail at once.

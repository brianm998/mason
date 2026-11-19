# 0594 — Gate the process-wide launch read on launch_frozen_alert

The step-host launch list is global to the process, so tests in one binary saw each other's
slow launches. `gather` now ignores it when `launch_frozen_alert` is false; the default (true)
is unchanged. The notify test filters claimed announcements by kind, as its helper already does.

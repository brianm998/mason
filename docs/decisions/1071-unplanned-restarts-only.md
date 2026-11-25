# 1071: unplanned restarts only

The ticket suggested excluding starts after an `update_ok` row, but that row is only written after an `update_failed` recovery. Instead `take_over` records `planned` before the exec (and `cancelled` if it fails), and the restart count skips the `start` that follows a `planned`. Crashes, rollbacks and reboots still count; the severe threshold stays at 3.

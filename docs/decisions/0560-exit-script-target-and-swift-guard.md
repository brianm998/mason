# 0560: exit scripts follow test module locations and the Swift skip convention

Exit scripts name tests by where they live now (`--test it landing_split::`), and any script that runs Swift carries the `MASON_SKIP_SWIFT` / Darwin / `swift` guard. Swift still runs on macOS through monitor-swift and the nightly macOS job.

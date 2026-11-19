# 0619: exit scripts follow real paths; the test-count gate is platform-aware

Exit scripts follow the iOS icon to its asset-catalog path (decision 0454). The s475 count gate stays, but expects 931 on macOS and 927 elsewhere, requiring the four macOS-only tests present or absent accordingly, so it still catches real drops.

The executable cap in s475 was 13, but develop already builds 20 test executables (one per top-level file in crates/daemon/tests plus lib and bins), so it failed on every platform. It is now 24: still far below one-per-test, with headroom for a few new files.

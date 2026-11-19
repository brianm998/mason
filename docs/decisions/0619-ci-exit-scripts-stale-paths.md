# 0619: exit scripts follow real paths; the test-count gate is platform-aware

Exit scripts follow the iOS icon to its asset-catalog path (decision 0454). The s475 count gate stays, but expects 931 on macOS and 927 elsewhere, requiring the four macOS-only tests present or absent accordingly, so it still catches real drops.

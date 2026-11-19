# 0584: Home's CPU and disk sections share a row by a custom Layout on width

The Swift home page puts CPU and disk side by side only when the width offered is at least 900 pt (the Compose threshold) and the screen is not a phone (`compact`); otherwise they stack, CPU first. A custom `Layout` (`CpuDiskRow`) decides from the proposed width, so an iPad in a narrow split and a Mac window narrowed by the sidebar both stack without a size-class read. The split is 3:2 (`Disk.cpuWeight`, `Disk.diskWeight`), as in Compose.

The hidden volumes are stored in the preference `disk.hidden`, one id per line, as in Compose, and pruned when the daemon stops listing a volume.

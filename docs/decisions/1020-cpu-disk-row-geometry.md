# 1020: CPU/disk row geometry is the assertion, not pixel colour

## Context

The DiskSectionTests layout tests sampled ImageRenderer pixels for red and blue boxes. The rendering differs between runner images, so they failed on CI (run 45005421555) while the layout was right.

## Decision

Layout is asserted on frames from the pure `CpuDiskRow.placement`, which the Layout itself uses. Rejected: more tolerant colour probes, which still depend on the renderer. A failing test's name is confirmed from the full log before a fix; a log tail is not enough (decision 0905).

# 0556: isExpanded read-count test baselines after construction

Status: accepted (s563).

Context: `isExpandedReadsEachKeyOnce` summed all preference reads, including the mute read the `MonitorModel` constructor makes, and failed on base with 2 reads where it expected 1.

Decision: the test takes the read count after construction and asserts that `isExpanded` and `setExpanded` add exactly one read in total. The product code stays unchanged. Rejected: making the mute read lazy, which changes startup for no gain.

Trust: no protected path is touched.

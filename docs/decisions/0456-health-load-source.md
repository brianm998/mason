# 0456: The load finding names its CPU source and does not report a placeholder as a measurement

When the sampler has no fresh reading and the load average fails, it answers 100 with source `assumed-busy`. The finding "overloaded:load" used to read "the machine is at 100% load" then. Now `Facts.load_source` carries the source: for `assumed-busy` the finding says "CPU unreadable"; otherwise the wording is kept and the evidence says "CPU reading from <source>".

The threshold (`Limits.load_pct` = 95) and the detector are kept, so the finding still fires on `assumed-busy`; it is just worded honestly. Percentage and source come from one `CpuSource::reading` call.

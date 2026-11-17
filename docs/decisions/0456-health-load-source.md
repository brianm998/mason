# 0456: the load finding names its CPU source

`Facts.load_source` carries where the load reading came from. The "overloaded:load" finding adds "CPU reading from <source>" as evidence. A reading with source `assumed-busy` is a placeholder, so (as 0458 decided) no finding is raised for it. The threshold (`Limits.load_pct` = 95) is unchanged. Percentage and source come from one `CpuSource::reading` call.

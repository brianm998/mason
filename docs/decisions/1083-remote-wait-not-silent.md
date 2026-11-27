# 1083: a live remote run excuses a step's silence

The remote poll marks each successful look at a running run, rather than posting step progress: the monitor's progress text stays as it is, and the excuse can be bounded. It holds only once a runner has the run and until pickup plus the runner limit plus 10 minutes (the poll cancels at the limit and then collects the output). Silence counts from the last successful poll, so polls that keep failing are still reported after 30 minutes. A run still queued on GitHub or waiting for a pool unit is treated as before, and so are runs on nodes.

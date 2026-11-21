# 0715: One decision says where a run goes; hard local reasons beat every switch

`place::decide` answers from plain facts: a task's own choice (#699), else the global switch in `remote.toml`. A run that would go to GitHub still stays on this Mac when the project has no `[remote_runs]`, the suite says `remote = false`, its environment leases a device, it uses `exclusive`, or the remote setup check failed, and the answer carries the reason. So even a task that asks for GitHub cannot move a simulator suite off the machine that has the simulator. A task that asks for local, and a global switch that is off, are answered first and say so.

The switch is a file read on each decision (not state in the daemon) so `mason remote on` takes effect for the next run with no restart and no reload, and a hand edit works the same. Each change records who and when in the file and a log. A broken file is local, and says so in `mason remote status`, because a bad switch must never send work off the machine.

A suite whose `build` goes to GitHub has no local build: the remote command is `build && command`, since a build on this Mac would be wasted on a runner that cannot see it. Landing, bisects, base runs and debts are not asked and stay here until #702.

# 0582: count a surviving host's slots from its own directory

**Decision.** The new daemon adopts the `uses` recorded in each running step host's directory as held leases until that host's command ends. The alternative, handing the kernel lock fds to the host, was rejected.

**Why.** The host starts after the lease is granted and is launched detached, so passing lock fds would change the launch path; a host holding flocks would also make `count_outside_holders` count them as outside holders of a task the broker already knows. Recording the holds needs no change to the launch path, and the host list is already how a takeover finds surviving hosts. Nothing is granted before adoption because `adopt_hosts` runs before the broker is given to anything.

**Limits kept exact.** Adopted holders count against capacity like any lease and are released only when the host's command has ended. They are excluded from the kernel comparison, so outside-holder counting is unchanged.

**Cost.** Slots are counted by recorded intent, not by a kernel lock, so a host with no record (started by an older daemon) is not counted.

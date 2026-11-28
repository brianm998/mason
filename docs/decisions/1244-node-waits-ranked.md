# 1244: a run waiting for a node takes its unit through the broker

Decision: a nodes-only run that finds no node with room asks the broker for a unit of the node
`nodewait::pick` names, in the same ranked line as every other request for that node, instead of
polling the nodes itself. Why: the poll gave it no rank, no wait event and nothing to show, and
runs that held node units could take every slot ahead of it for ever (trd #1029, 2026-11-27).

How ranking works: the unit is a `node:<nid>` pool request like any other, so the broker's
ordinary rank applies (progress, then priority, then age; a request that has waited past
`order.max_wait_s` ages ahead of equal-progress work). A short suite cannot be passed over for
ever, and later-arriving work does not pass it. `pick` takes a node that is taking work and has
the suite's toolchains, the one with most jobs free, then the lowest id. The run is not moved to
another node that frees first; it waits for the node it asked for.

When it cannot hold a unit: with `[nodes]` off, the project not allowed, no node paired, every
node away or not taking work, or no node with the toolchain, no node could ever take the run. It
holds no unit (a unit held for a node it cannot use would starve runs that can) and waits in the
node executor, which puts a wait on the task (`WaitStarted`, class `environment`) saying which
of those it is. A node full of another coordinator's jobs shows as full in its status; the pool
reads that and the run waits in line. A node that gives a job back is rested and the wait reads
"node full".

What the page says, for each part not running: why (node full, node off, toolchain missing,
project may not run elsewhere), who holds the slots and for how long, its place in line and an
expected wait. The expected wait is the `p`-th smallest of what is left of the holders' usual
run times (`suite_times`, phase `run`), plus the longest usual hold for each full round of runs
ahead of the holders. `no estimate` when any holder has no history. It is an estimate from past
runs, never a promise. Both monitors and `mason task show` read the same sentence.

Pool size: `nodes::set_capacity` set the limit to jobs free + units held, which counts a unit
leased but not yet running at the node twice (seven units on a six-job node). It is now capped at
the node's own `max` jobs (held units are never taken back). This lowers a limit that was wrong;
no limit was raised or removed.

Trust: `crates/daemon/src/sched.rs`, `suites.rs`, `crates/resources` and `crates/scheduler` are
not changed. Only `remote.rs`, `hostedcheck.rs`, `node_exec.rs`, `nodes.rs`, new `nodewait.rs`,
`views.rs`, `http.rs` and the CLI are.

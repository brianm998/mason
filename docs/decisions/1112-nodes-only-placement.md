# 1112: nodes-only placement for projects without `[remote_runs]`

## Context

A node needs no GitHub, but the only way to send a project off the coordinator was its `[remote_runs]` section, where the placement mode also lived.

## Decision

- The mode may live in `[placement] mode`. A project with no `[remote_runs]` and no mode behaves as before. Such a project never follows the global mode into waiting: only a mode of its own (or a task's choice over it) does.
- In `remote` mode "off this machine" is meant strictly: with no node that has room the run waits, for as long as its step wants it. It is not turned into an environmental failure that would fall back here.
- The wait is in the node executor, carried by a mark on the node id (`wait:`), so the suite runner and verdict code (`suites.rs`, protected) need no change and cannot tell where a run was.
- A nodes-only run on a node holds no slot of the coordinator. The owner moved work off that machine for heat; holding its slots (and so its CPU and thermal holds) for work done elsewhere would keep the projects stalled. The node refuses jobs it has no room for, which bounds what is sent.
- Projects with `[remote_runs]` keep their behaviour, including how their node runs hold slots.
- Clearing the mode of a project without `[remote_runs]` returns it to "always runs here"; it has no "follow MASON".

## Consequences

If nodes are off, or no node has the toolchains, a `remote` nodes-only project waits and only the daemon log and `mason task show` say why. Use `favor-remote` where running here is better than waiting.

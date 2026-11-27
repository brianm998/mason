# 1164: Hosts section reads `HomeView.hosts`; its absence marks an old daemon

## Context

#1155 added `HomeView.hosts` and `TaskDetailView.running_on`. The monitor had GitHub and Nodes sections but nothing grouped by host.

## Decision

Show a Hosts section from `hosts` as sent (the daemon applies the project filter). Hide it when `hosts` is empty, or when no host has a run, there are no nodes and GitHub runs are off. Show "Running on" on the task page only when `hosts` is non-empty, since protobuf cannot tell an empty `running_on` from a missing one. Text lives in `MonitorCore/Hosts.swift` so it is unit-tested.

## Consequences

- Compose does not get it; `features.toml` records that (owner approval needed for that protected file).
- `PlacementControl` stays: it is the mode, not the location.

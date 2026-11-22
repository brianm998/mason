# 0869: Settings gets a "Where runs go" section from the existing /remote API

## Context
The placement mode could only be seen and changed with `mason remote`. The daemon already serves `GET /remote` and owner-only `POST /remote`.

## Decision
Both monitors' Settings pages read `/remote` and write it, with no daemon change. The project choice is disabled when the project file has no `[remote_runs]`, since the daemon refuses it. No "follow the global mode" choice yet.

## Consequences
A refusal shows the daemon's message in the settings notice. `clients/features.toml` changes, so the owner approves it.

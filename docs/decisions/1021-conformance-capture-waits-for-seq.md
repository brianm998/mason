# 1021: conformance capture waits for the daemon's seq

## Context

The Compose and Swift capturers settled on quiet time alone and could capture before a step's views arrived.

## Decision

Both read the seq from GET /status and wait for it, with a 1.5 s quiet fallback and a 60 s failing hard limit, as `crates/conformance` does. Rejected: longer quiet times, which are timing guesses.

## Trust

No protected paths.

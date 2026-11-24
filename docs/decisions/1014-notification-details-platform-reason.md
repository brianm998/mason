# 1014: record a platform_reason for notification-details

## Context

Lint fails at `conformance features clients/features.toml`: the notification-details row has swift and compose done, ios todo, and no `platform_reason` (check at `crates/conformance/src/features.rs:136-144`).

## Decision

Record the reason in the row. Rejected: relaxing the check, or building the iOS version now.

## Trust

Touches `clients/features.toml` (PROTECTED, client parity gate); owner approval needed.

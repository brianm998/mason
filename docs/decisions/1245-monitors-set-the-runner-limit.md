# 1245: monitors set the GitHub runner limit; `POST /resources/limit` is owner only

Decision: the monitors change `gha` and `gha-macos` with the existing `SetLimit` command, and
`POST /resources/limit` now refuses anyone but the owner. `gha_ceiling`, `gha_macos_ceiling`,
`gha-debt` and `gha_full_share` stay read-only in the monitors: the ceiling is the account's fact
and the owner's deliberate CLI setting, and the others are tuning that a plus button invites.

Trust finding: before this change `resource_limit` in `crates/daemon/src/http.rs` had no owner
check, unlike `resource_gate`. A LAN viewer is refused earlier (`mason_core::access::decide`: a
viewer changes nothing) and a LAN owner's identity is the token's, but a local WebSocket client, an
agent's `mason resources limit`, or any local `Raw` call with no `x-mason-as` could change any
limit. It now needs the owner's identity on every path (WebSocket `SetLimit`, `Raw`, the CLI);
the CLI defaults `MASON_AS` to `$USER` for it, as for `gate`. The owner names default to `$USER`, so
existing scripts that run `mason resources limit` as the owner keep working; a script run as
another user without `MASON_AS` is refused.

The control keeps the limit between 0 and the ceiling, says at 0 that no check goes to GitHub, and
says that lowering stops new grants only. `sched.rs` (protected) is unchanged; enforcement is as before.
The view reads the broker's snapshot; the daemon accepts values above the ceiling from the CLI
(unchanged), and the monitor shows both numbers when the ceiling caps the limit.

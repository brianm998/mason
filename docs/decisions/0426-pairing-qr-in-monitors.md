# 0426: The monitors draw the pairing QR code themselves

Decision: the daemon is unchanged. Each desktop monitor calls the existing loopback API (`GET /lan`, `POST /pair/codes`, `GET /devices`, `POST /devices/{id}/revoke`) and draws the returned `mason://pair` URL as a QR code. Swift uses CoreImage; Compose uses a small pure-Kotlin encoder in the core (`QrCode.kt`), because no QR library is among the build's dependencies and adding one would need an approval.

The list refreshes by polling `/devices` every 3 s while the pairing sheet is open, rather than a new protocol event: it costs one cheap GET and no daemon change. The countdown is computed from the time the code arrived and an injected clock, so it is testable without timing.

Rejected: shelling out to `mason pair` (parses terminal output, needs the CLI on the path); a protocol event for "device paired" (daemon and schema change for a sheet that is open for a minute).

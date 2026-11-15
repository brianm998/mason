# 0453: Push to paired iPhones and iPads goes straight from the daemon to APNs

The owner builds the iOS monitor with their own Apple developer team, so the daemon can sign APNs requests itself with the owner's `.p8` key and send to Apple with no relay. `apns` is a new channel beside `push` (ntfy), routed like any other. It is on by default for `needs_you`, `failed`, `landing`, `stalled` and `daemon_down` at 0 minutes, and does nothing until `[apns]` is set up and a device has registered a token (a routed channel that is not set up is skipped, as before).

**Config** (`notify.toml`): `[apns] key_file, key_id, team_id, topic`, optional `viewers = true`, and `endpoint` (replaces both Apple hosts; used by tests and the exit script, so `netguard.rs` is not touched: in test mode `Curl` refuses any non-loopback URL as before). A key file with any group or other permission bits is refused when it is read, with an error that names the mode and never the key.

**Sending**: the ES256 JWT (`kid` = key id, `iss` = team id, `iat`) is made with `ring` and cached 50 minutes (`adapter-api/src/apns.rs`). One HTTP/2 POST per device through the system `curl` (`Curl { http2: true }`): the JWT is a header in the config on curl's stdin, never on a command line or in an error text. With `http2` set, `/usr/bin/curl` is preferred over the first `curl` on PATH, because MacPorts' 7.46 on this machine answers "Unsupported protocol" to `http2`. `410` and `400 BadDeviceToken` clear that device's token (only if it is still the one that failed); `429` and `5xx` are transient; `403 ExpiredProviderToken` drops the cached JWT and is retried. A delivery counts as made when any device took it.

**Who**: owner devices; viewers too with `viewers = true`. Tokens live in nullable `devices.push_token` / `push_env` (migration 41). Revoking a device clears its token in the same statement.

**Why `/devices/self/push` is safe for a viewer** (`crates/core/src/access.rs`): the route names no device. The handler takes the row from the LAN guard's `Peer`, which is set from the bearer token, and ignores any `device` or `id` in the body (a test sends one). The only fields are a hex token and `sandbox`/`production`. It is the one exception to "a viewer changes nothing", and it changes only the caller's own row, so a viewer cannot redirect anyone else's pushes; the worst it can do is stop or misdirect pushes to itself. Only `POST` and `DELETE` on that exact path are allowed (a test checks near misses). Without a `Peer` (a request not from a paired device) it is 403, and an unpaired LAN caller gets 401 from the guard.

**Limits kept**: a daemon that is down cannot send its own "daemon down" push; `scripts/mason-watchdog.sh` and ntfy remain the way to hear that.

Rejected: a relay (needs credentials tied to one publisher); putting the token in the alert row (it belongs to the device, and revoke must clear it); making `apns` a mode of `push` (different config, targets and answers).

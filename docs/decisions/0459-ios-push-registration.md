# 0459: iOS push registration

Status: accepted

- The token is hex-encoded and sent with `env` chosen at compile time (`DEBUG` is `sandbox`). Reason: the entitlement and the token's host match the build, and a runtime guess could send a sandbox token to the production host.
- The permission prompt follows an explanation screen shown once; "Not now" leaves notifications off until Settings turns them on. Reason: the system prompt can be answered only once.
- What was sent last is remembered as `env:token@fingerprint`, so an unchanged token is not re-sent but a new pairing or token is.
- Forget sends DELETE before it clears the token, since the call needs it; a revoked token skips it. If the DELETE fails the daemon keeps a token it will clear itself when Apple answers 410.
- "Send a test" uses the daemon's existing `POST /notify/test`, which reaches all registered devices; a per-device route was not added.

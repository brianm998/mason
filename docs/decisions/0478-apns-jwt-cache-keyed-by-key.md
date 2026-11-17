# 0478: The APNs token cache is keyed by the signing key

**Decision.** `cached_jwt` reuses a token only when the key (held as a SHA-256 digest), key id and team id all match and the token is under 50 minutes old.

**Why.** The cache is process-wide. Keyed on key id and team id alone, a replaced key file with the same key id kept signing with the old key until the TTL ran out, and tests that share ids but use different keys got each other's tokens (the flaky `a_push_reaches_owner_devices_...`).

**Rejected.** Serializing or ignoring the daemon tests: it would hide a real bug. Keeping the PEM text in the static: a digest is enough and keeps the secret out of a long-lived static.

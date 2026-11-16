# 0477: the APNs provider-token cache is keyed on the key

Status: accepted

`cached_jwt` kept one static token and reused it when the key id, team id and age matched. Two different private keys registered under the same ids therefore shared a token signed by whichever key sent first, which broke `a_push_reaches_owner_devices_with_a_jwt_that_verifies_and_skips_viewers` when the daemon's apns tests ran in parallel. The cache now also holds a SHA-256 fingerprint of the key PEM and reuses a token only when it matches. The key text itself is never stored in the static.

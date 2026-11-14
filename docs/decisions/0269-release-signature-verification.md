# 0269: Verify a minisign signature on SHA256SUMS, fail closed once a key is pinned

Context: SHA256SUMS shows a download matches the release, not that the release is the owner's.

Decision: sign SHA256SUMS with minisign in `publish`. The public key is pinned in `scripts/install.sh` and `crates/daemon/src/release.rs`. Until the owner supplies it a placeholder stands there; the placeholder is never passed to minisign, and while it is the only key neither check can verify a signature, so both say so and carry on with the checksum alone. Once a key is pinned (or `MASON_MINISIGN_PUBKEY` is set) a missing signature, a bad one, a malformed key or a missing `minisign` refuses the install or update. The signature is checked before SHA256SUMS is believed.

Why the system `minisign` and not a Rust crate: a signature crate adds to `deny.toml`'s dependency tree, which only the owner changes; the update channel already shells out to `curl` and `tar`. Cost: a signed update needs `minisign` installed.

Consequences: pinning the key before a signed release exists makes every install and update fail; docs/RELEASING.md says to sign first. Two copies of the key must match. Owner approval is needed for the workflow and the key.

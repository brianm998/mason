# 1194: Advertise through the system mDNS responder on macOS

Status: accepted

`masond` ran its own `mdns-sd` responder with the Mac's `<gethostname>.local.` as the SRV host and automatic addresses, so it answered for a name that belongs to the system's mDNSResponder. macOS saw a second responder, treated it as a conflict and renamed the Mac at every boot.

- **macOS:** `_mason._tcp` is registered with `DNSServiceRegister` (libSystem, a small FFI module `dnssd.rs`, no new crate, so `deny.toml` is untouched) with a null host and domain. The system answers for its own current name. The ref lives for the process; a thread runs `DNSServiceProcessResult`, and `advertise` waits (bounded, 5 s) for the first registration callback so a refusal reaches `node doctor` as `Mdns::Failed`.
- **Other platforms:** `mdns-sd` stays, with a host name of our own, `mason-<first 8 of the fingerprint>.local.`, never the machine's. Avahi was not used because a build must not depend on its daemon being present.
- `lan::hostname()` reads `LocalHostName` (`scutil --get LocalHostName`, cached 30 s) on macOS and falls back to `gethostname`. The certificate of an existing install is not regenerated; only new identities pick up the new name.
- `mason node doctor` reports the advertised name and says when `gethostname` differs from `LocalHostName`.
- Clients (`Remote.swift`, `Pairing.kt`) read the SRV target and fall back to `<host>.local`; not changed, and not run against a system-registered node.

Not verified: the reboot-and-login rename alert itself, and coordinator discovery of a system-registered node over a real network. Owner check: reboot the node Mac, confirm no alert and the same `LocalHostName`.

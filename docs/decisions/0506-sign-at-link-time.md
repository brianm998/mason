# 0506: sign Rust outputs at link time and sign the monitor app

**Decision.** `.cargo/config.toml` passes `-Wl,-adhoc_codesign` for `x86_64-apple-darwin`, and the swift monitor is built with `SIGN_APP=1` and an ad hoc identity unless `MASON_SIGN_IDENTITY` is set.

**Why.** Unsigned Mach-O files deleted during a `syspolicyd` scan crashed it (2026-11-08), and launchd then holds it down for up to 20 minutes. Signing does not cut its CPU (s502 measured that); this is only about the crash.

**Why a config file.** A `CARGO_*`/`RUSTFLAGS` variable breaks sccache's cache; a target rustflags entry applies to every build alike.

**Trust.** `.cargo/config.toml` is the owner's to approve. The one-time rebuild cost is unmeasured.

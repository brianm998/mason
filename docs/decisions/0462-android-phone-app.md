# 0462: The Android phone app is in the build, with Keystore tokens, a fourth parity column and no push

Status: accepted (owner approval needed: `clients/features.toml`, `crates/conformance/src/features.rs`, `.github/workflows/android.yml`, the new exit script and the one assertion changed in `scripts/exit/s351.sh`)

## Decision

1. **The module is built.** `clients/compose/android` is an Android application (Android Gradle plugin 8.5.2, compileSdk 35, minSdk 29, portrait) that shares `:core`: the model, `PhoneLayout`/`PhoneNav`, `PairUrl`, `Pin`, `Reach.order`, `MonitorConnection`. The application id is the Gradle property `mason.android.applicationId` (placeholder `dev.mason.android`), as the iOS bundle id is a build setting.
2. **It is in the build only on request** (`-Pmason.android=true`). Every other Gradle run, including the desktop monitor's and the many exit scripts that call it, never configures the module, so no machine needs the Android SDK or the plugin to build the desktop monitor. The plugin is put on the root build's classpath only then, because the Kotlin plugin must share a class loader with it.
3. **Tokens are sealed with an Android Keystore key** (AES-256-GCM, made on first use, never exported), stored per fingerprint. A token whose key has gone, or that was damaged, reads as no token and the phone pairs again. The sealing sits behind `SecretBox`, so the unit tests use a plain AES key.
4. **Connection**: NSD finds the daemon whose fingerprint is the paired one (other fingerprints are ignored), then `Reach.order` walks the pairing host and the extra hosts. A failed attempt moves to the next address. The socket is `wss`, pinned to the fingerprint (`PinningTrustManager`; the name is not checked, the pin is), with the token as a bearer on the upgrade. The socket is closed in the background; coming back says hello with the last sequence number. A 401 on the upgrade clears the token and the pairing.
5. **Pairing**: a CameraX preview read with ZXing (`com.google.zxing:core`, Apache-2.0), or a nearby daemon and its 8-digit code, with the same warning as the iPhone that scanning is the stronger check; `mason://pair` links; Forget. **ZXing, not ML Kit**: ML Kit's barcode library is under Google's proprietary SDK terms, which sit badly with GPL-3.0-or-later; ZXing is Apache-2.0 and needs no Play services. `deny.toml` covers Cargo crates only, so it needs no change.
6. **Parity**: `android` is a fourth column. `done`, `todo`, `n/a` or `difference`; a `difference` names a `[phone.*]` table in `android_phone`; a `todo` or `n/a` needs its own `android_reason`, so every missing feature is a recorded debt with a stated reason, not a silent one. An Android `done` where swift is not done still needs a `platform_reason`. The first Android app has 12 features done, 3 not applicable on a phone, 3 phone differences and 56 recorded todos.
7. **No background alerts on Android in this decision.** Native push needs Firebase Cloud Messaging and a Firebase project, which the owner has not chosen. Until then an Android phone gets background alerts only through the ntfy channel (decision 0453 covers the iOS push channel; ntfy is stage 23). Nothing here adds a Firebase dependency or a Google services file.

## Why

The design promised an Android monitor, but the module could not be built and held no screens. The iPhone app (decisions 0452, 0453, 0460) is the model for the screens; the Compose core already held the logic.

## Not verified

See `docs/stages/s469.md`: no real phone, no emulator, no camera, no NSD on a network, and the Compose screens were compiled but not looked at.

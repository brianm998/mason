# 0082: One copy of the mascot art, one manifest, a loader that does not use `Bundle.module`

The art is stored once, in the SwiftPM target's `Art/` folder, and Compose reads that folder through `resources.srcDir`, so the two monitors cannot drift. `clients/mascot.toml` lists every id and file; `conformance mascot --check` fails on a missing or unlisted file and on stale generated names, like `tokens`.

SwiftPM's generated `Bundle.module` falls back to the absolute build-directory path and traps if that is gone, so the loader looks for the resource bundle itself (app Resources, beside the executable, beside the test bundle). `scripts/bundle-monitor.sh` copies the bundle into `MASON.app/Contents/Resources`. A guard-free change: no gate, budget or verdict logic is touched. The protected files `ci.yml` (gains one step) and `features.toml` (gains one feature) only add checks.

Unknown ids and unsupported sizes return nothing; icons exist at 20 and 24 only.

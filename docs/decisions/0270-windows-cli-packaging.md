# 0270: Windows CLI packages are a PowerShell script plus a WiX v3 source, static CRT

**Decision.** `scripts/package/windows.ps1` builds with `RUSTFLAGS=-C target-feature=+crt-static`, zips the five exes, `mason-askpass.cmd` and licences, and runs `candle`/`light` on `packaging/windows/mason.wxs`. The MSI is per-user (`%LOCALAPPDATA%\Programs\MASON`), appends to the user PATH, and has a fixed UpgradeCode. `windows-e2e` fails on any imported VC runtime/UCRT DLL or non-system DLL.

**Why.** WiX v3 is preinstalled on windows-2022 runners, so no download step; the static CRT avoids shipping a redistributable. A hand-written .wxs keeps the upgrade code and PATH behaviour explicit, unlike cargo-wix.

**Trust.** `.github/workflows/release.yml` is under the protected `.github/workflows/**`: owner approval needed. `scripts/exit/s268.sh` changed only to drop the check for the now-removed gap marker.

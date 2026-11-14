# Releasing MASON

Releases are built by GitHub Actions on a pushed tag `vX.Y.Z` (the `stable` update channel and
`scripts/install.sh` expect `v*`). The repository is <https://github.com/brianm998/mason>.
The version is the workspace version in `Cargo.toml`; `scripts/release-version.sh <tag>` prints
it and fails when the tag is malformed or differs from `Cargo.toml`. The Swift monitor
(`scripts/bundle-monitor.sh`, or `MASON_VERSION`) and the Compose monitor (`-Pmason.version`)
take the same version. `mason --version` and `masond --version` print
`mason <version> (<commit>)`; the commit comes from `MASON_BUILD_SHA`, else git, else `unknown`.

## Asset naming contract

Every packaging job and the download page use these names, with `<ver>` the version without
the `v`. Nothing else is published under a release.

| Asset | Contents |
|---|---|
| `mason-<ver>-<target>.tar.gz`, target in `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu` | `mason`, `masond`, `step-host`, `mason-mcp`, `fake-claude`, `mason-askpass.sh`, the LICENSE files, README |
| `mason-<ver>-x86_64-pc-windows-msvc.zip` | CLI + daemon, Windows |
| `mason-<ver>-x86_64-windows-setup.msi` | CLI + daemon, Windows installer |
| `mason_<ver>_amd64.deb`, `mason_<ver>_arm64.deb` | CLI + daemon, Linux |
| `mason-<ver>-macos.pkg` | CLI + daemon, universal, signed + notarized |
| `MASON-<ver>-macos.dmg` | Swift monitor, universal, signed + notarized |
| `mason-monitor_<ver>_amd64.deb` | Compose monitor, Linux |
| `MASON-Monitor-<ver>-windows.msi` | Compose monitor, Windows |
| `SHA256SUMS` (later also `SHA256SUMS.minisig`) | SHA-256 of every other asset |

## Package layout

Built by `scripts/package/tarball.sh <target> [--out DIR]`, `deb.sh <target>` and
`checksums.sh DIR`.

- Tarball: one top-level folder `mason-<ver>-<target>/` holding `mason`, `masond`, `step-host`,
  `mason-mcp`, `fake-claude` (the self-test of `mason update` needs it next to `masond`), `mason-askpass.sh`, `LICENSE`, `LICENSE-ADDITIONAL-PERMISSION` and `README.md`.
  Binaries are stripped; on macOS each is signed ad hoc, or with `MASON_CODESIGN_IDENTITY`.
- `.deb`: the four binaries and `mason-askpass.sh` in `/usr/bin`, `Depends: git`,
  `/usr/share/doc/mason/copyright`; the postinst only prints `mason service install`.
- `SHA256SUMS`: `<hash>  <file name>` lines for every asset in the folder.

## Cutting a release

1. Bump `version` under `[workspace.package]` in `Cargo.toml` (and the path-dependency
   versions in the crates), update `Cargo.lock`, merge to `develop`.
2. Tag the merge commit `vX.Y.Z` and push the tag.
3. `.github/workflows/release.yml` runs `scripts/release-version.sh` first, then builds every
   asset above, installs each one on a fresh runner (`*-e2e` jobs), and in `publish` drops any
   asset whose end-to-end job did not succeed (an `::error::` names it), writes `SHA256SUMS`,
   creates a draft release `MASON <ver>` with generated notes, and fails if an asset from the
   table is missing. Until the Windows packaging jobs are added (the `WINDOWS-CLI-JOBS` mark in
   the workflow) the two Windows CLI assets are missing and `publish` fails after creating the
   draft; that is intended, so nothing incomplete is mistaken for a release.
4. Review the draft: all assets present, `SHA256SUMS` matches, installers start.
5. Publish the release.

### Secrets

Repository secrets for the signed macOS builds (copy them from the star repository's settings):
`APPLE_CERTIFICATE_P12_BASE64` (a .p12 holding the Developer ID Application and Installer
identities), `APPLE_CERTIFICATE_PASSWORD`, `KEYCHAIN_PASSWORD`, `APPLE_API_KEY_P8_BASE64`,
`APPLE_API_KEY_ID`, `APPLE_API_ISSUER_ID`, `APPLE_TEAM_ID`. Without them the macOS packages are
built unsigned and `publish` removes them and reports an error, so an unsigned build is never
attached to a release.

### Dry run

Actions, Release, Run workflow (`workflow_dispatch`) on any branch. It builds, signs and tests
everything with the workspace version and creates no release (`publish` runs on tags only).
Do this once after landing changes to the workflow, before the first real tag.
The workflow has not been run yet.

## macOS monitor dmg

`scripts/package/macos-dmg.sh` builds `MASON-<ver>-macos.dmg`. Set `SIGN_APP=1` and
`SIGN_IDENTITY` ("Developer ID Application: ...") to sign with the hardened runtime, and
notarization credentials (`NOTARY_PROFILE`, or `APPLE_ID`, `APPLE_TEAM_ID`, `APPLE_APP_PASSWORD`)
to notarize and staple through `scripts/package/notarize.sh`. No entitlements are needed.

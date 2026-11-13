# 0236: A `release` update channel for packaged installs

**Status:** accepted (task #233)

**Problem.** Self-update builds from source (0013). A packaged install has no checkout and
usually no Rust toolchain, so `mason update` and the monitors' banner could do nothing for it.

**Decision.**
- New channel `release` (`channel = "release"` in `update.toml`, or `--channel release`). It asks
  `<release_api>/releases/latest` (default `https://api.github.com/repos/brianm998/mason`; a
  `release_api` key changes it, which is how the tests use a `file://` fake), picks
  `mason-<ver>-<target>.tar.gz` for this OS and architecture (docs/RELEASING.md), downloads it
  and `SHA256SUMS`, and refuses unless the tarball's SHA-256 is listed and equal. It unpacks into
  `runtimes/<ver>.partial/`, signs the binaries as source builds are, writes `SHA` and `READY`
  and renames to `runtimes/<ver>/` (complete or absent), then runs the new `masond selftest` on a
  copy of the database and takes over exactly as before. Any failure leaves the running daemon
  alone. `--sha <ver>` installs a named release (`/releases/tags/v<ver>`).
- Versions replace the ancestor check: `X.Y.Z[-pre]` compared as numbers, a pre-release before its
  release. Older needs `--rollback`; a running version that is not a release (a source commit)
  cannot be compared and also needs `--rollback`.
- A `masond` that is not inside a cargo `target` directory, with no `update.toml`, defaults to the
  `release` channel. Source installs are unchanged. A daemon not yet running from a runtime
  counts its own crate version as the running one.
- `UpdateInfo` gained `release_version` and `download_url` (fields 4, 5). On this channel the
  monitors' daemon button reads "Download" and opens the page (`download_page`, default
  `https://brianm998.github.io/mason/`) instead of building.
- The code is split: `release.rs` does the look, download, checksum and unpacking;
  `update.rs::prepare_release` calls it and the shared tail (backup, selftest, takeover) is the
  one source updates use, so the Windows takeover and signed-checksum tasks touch separate places.

**Tarball contents.** `masond selftest` (protected; unchanged) runs a scripted task and fails
without `fake-claude` next to `masond`, so the tarball and `.deb` now ship `fake-claude`
(`scripts/package/lib.sh`, docs/RELEASING.md); the unpacker requires it. Tarballs built before
this change cannot be installed by `mason update`.

**HTTP.** The daemon has no HTTP client (only hyper's server), and adding a TLS client would
change deny.toml, a trust file. Transport is the system `curl`, as git is the system `git`
(curl ships with macOS, current Linux distributions and Windows 10+). curl is restricted to the
scheme of the configured API and redirects only to https; a stalled transfer ends after 60 s.

**Not done / not verified.** Windows (its asset is a zip and its takeover is another task):
`release` reports no tarball for Windows. Signature verification is the signed-checksums task's.
The Swift and Compose edits (a "Download" button) were written but not compiled here. The real
GitHub API was not contacted; the tests use a `file://` fake. `Update monitor` buttons are not
shown for release checks, since there are no per-monitor commits.

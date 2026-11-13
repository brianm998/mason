# 0218: release packaging is plain shell under scripts/package/

**Scripts, not a workflow.** `tarball.sh`, `deb.sh` and `checksums.sh` take a target or folder and print what they wrote, so the release workflow, the later .pkg task and a developer call the same code. Names follow `docs/RELEASING.md`.

**Layout.** The tarball has one top-level folder `mason-<ver>-<target>/` with the four binaries, `mason-askpass.sh` (`askpass_script()` looks beside the binary), both LICENSE files and README.md; `install.sh` finds binaries with `find`, so the folder is safe. The .deb puts the binaries and askpass in `/usr/bin`, depends on `git`, and its postinst only prints `mason service install`; it never starts or enables anything.

**Signing.** Stripping invalidates a macOS signature, so every Mach-O is re-signed after strip: ad hoc by default, a real identity from `MASON_CODESIGN_IDENTITY`.

**SHA256SUMS.** `<hash>  <name>` with names only, every regular file in the folder except `SHA256SUMS*`.

**Trust.** No PROTECTED path changes. `scripts/exit/s223.sh` is a new gate and waits for the owner.

**Not verified:** the .deb build (no dpkg-deb on this Mac), cross-target builds, notarization.

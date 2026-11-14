# 0235: The macOS package is one universal .pkg built from the tarball build

`macos-pkg.sh` builds each architecture with the tarball's `pkg_build`, joins them with `/usr/bin/lipo -create` (explicit path: MacPorts' lipo is first on PATH on the owner's machine), and signs the joined binaries again, since lipo output is unsigned. With `SIGN_APP` set it uses the hardened runtime and a secure timestamp (both required for notarization); otherwise ad hoc, so an unsigned package is valid for local tests. The payload installs to /usr/local/bin, askpass beside the binaries. `SIGN_PKG` runs `productsign`; nothing is read from files in the repository, only from the environment.

`notarize.sh` takes any .pkg or .dmg, so the monitor .dmg task shares it. It judges success from the JSON `status` (Accepted), not only the exit code, and prints the notary log otherwise.

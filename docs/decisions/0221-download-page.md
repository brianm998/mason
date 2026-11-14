# 0221: Download page from site/, picker in pure JavaScript

**Where.** `site/`, not `docs/`, because `docs/` holds design documents and Pages would publish them. Deployed by `pages.yml` with upload-pages-artifact and deploy-pages on a push touching `site/**` and on `release: published`.

**Picking.** The user agent is read in the browser (Pages is static). `pick.js` has no DOM access so node and jsc can test it. `navigator.userAgentData` supplies the architecture where present. Mac packages are universal, so arch only affects the Linux files and the mac tarball list. An unknown Linux architecture gets no main download, only the list of every Linux file. The Compose monitor ships for amd64 only, so arm64 Linux gets no GUI file rather than a wrong one.

**Installer one-liner.** The raw `develop` URL of `scripts/install.sh`, since no workflow publishes `install.sh` yet; no `brew` line until a tap exists.

**Trust.** `.github/workflows/pages.yml` is a protected path and waits for the owner's approval. No test, limit or gate is weakened. pages.yml also runs `scripts/tests/pick.sh` before deploying.

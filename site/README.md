# MASON download page

Static page for GitHub Pages (deployed by `.github/workflows/pages.yml`; the owner sets
Settings > Pages > Source to GitHub Actions). `pick.js` holds the pure selection functions
(`detectPlatform`, `pickAssets`, names from `docs/RELEASING.md`); `app.js` is the DOM glue.

Test hook: open `index.html?release=test/fixture.json` from `file://` to render the fixture
release instead of the GitHub API (Chrome needs `--allow-file-access-from-files` for `fetch` on
`file://`; a `.js` file that sets `window.MASON_FIXTURE_RELEASE` works without it). Add
`&ua=<url-encoded user agent>` to pretend to be another browser.

Tests: `bash scripts/tests/pick.sh` (node, else jsc). `assets/NOTICE.md` is the mascot art notice.

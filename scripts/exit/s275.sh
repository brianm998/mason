#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s275 exit test: the release workflow lints, includes the Windows jobs, and its asset list
# equals docs/RELEASING.md. The Windows jobs themselves are proven only by the owner's dry run.
set -euo pipefail
cd "$(dirname "$0")/../.."
if command -v actionlint >/dev/null 2>&1; then
  actionlint .github/workflows/release.yml
else
  echo "s275: actionlint is not installed; SKIPPED linting release.yml"
fi
bash scripts/tests/release-assets.sh
for j in windows-package windows-e2e; do grep -q "^  $j:" .github/workflows/release.yml; done
grep -q "windows-cli-zip:windows-e2e" .github/workflows/release.yml
grep -q "3571d89e-2987-428c-a619-e8d01075c3a6" packaging/windows/mason.wxs
grep -q "crt-static" scripts/package/windows.ps1
python3 -I -c "import xml.dom.minidom,sys; xml.dom.minidom.parse('packaging/windows/mason.wxs')"
echo "s275 OK"

#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s127 exit test: banner in the README, social card 1280 x 640, status block ok.
set -euo pipefail
cd "$(dirname "$0")/../.."
grep -q '^!\[.\+\](docs/images/readme_banner.png)' README.md
test -f docs/images/readme_banner.png
file docs/images/social_card.png | grep -q '1280 x 640'
bash scripts/check-status.sh
echo "s127 ok"

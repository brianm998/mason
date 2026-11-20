#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Exit test for s721: every daemon test is in exactly one CI shard, and ci.yml parses.
set -euo pipefail
cd "$(dirname "$0")/../.."
bash scripts/check-daemon-shards.sh 3
python3 -c 'import sys,yaml; yaml.safe_load(open(".github/workflows/ci.yml"))' 2>/dev/null \
  || ruby -ryaml -e 'YAML.load_file(".github/workflows/ci.yml")'
grep -q 'daemon-shard.sh' .github/workflows/ci.yml

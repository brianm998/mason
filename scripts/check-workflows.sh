#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Parse every workflow file; a file GitHub cannot parse fails instantly on every push (s808).
# Usage: check-workflows.sh [file...]   (default: .github/workflows/*.yml templates/github/*.yml)
set -euo pipefail
cd "$(git rev-parse --show-toplevel 2>/dev/null || dirname "$0"/..)" 2>/dev/null || true
if [ "$#" -eq 0 ]; then set -- .github/workflows/*.yml templates/github/*.yml; fi
if command -v ruby >/dev/null 2>&1 && ruby -ryaml -e '' 2>/dev/null; then
  ruby -ryaml -e 'bad = 0
ARGV.each { |f| begin; YAML.safe_load(File.read(f), aliases: true); rescue Exception => e; warn "FAILED: #{f}: #{e.message}"; bad += 1; end }
exit(bad == 0 ? 0 : 1)' "$@"
elif python3 -c 'import yaml' 2>/dev/null; then
  python3 -I -c 'import sys, yaml
bad = 0
for f in sys.argv[1:]:
    try:
        yaml.safe_load(open(f))
    except Exception as e:
        print("FAILED: %s: %s" % (f, e), file=sys.stderr); bad += 1
sys.exit(1 if bad else 0)' "$@"
else
  echo "FAILED: neither ruby (psych) nor python3 yaml is available to parse workflows" >&2
  exit 1
fi
echo "workflows parse ok"

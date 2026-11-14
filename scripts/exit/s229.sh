#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s229 exit test: the compose-clients suite config still reads Gradle's JUnit XML and forces the run.
set -euo pipefail
cd "$(dirname "$0")/../.."
grep -q 'gradle cleanTest test' .mason/mason.toml
grep -q 'parser = "junit"' .mason/mason.toml
echo "s229: ok"

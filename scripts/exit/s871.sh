#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s871 exit test: narrow_each takes {file} and {path}; `definition check` warns for pytest with plain {}.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-testing --lib -- narrow::tests::file_and_path_placeholders narrow::tests::a_split_unit_gives
cargo test -q -p mason-definition --test testing_tables -- a_pytest_suite_narrowed
dir=$(mktemp -d)
trap 'rm -rf "$dir"' EXIT
mkdir -p "$dir/.mason"
mk() {
  cat > "$dir/.mason/$1" <<TOML
schema = 1
start = "test"

[step.test]
type = "test"
category = "testing"
suites = ["py"]
next = "land"

[step.land]
type = "landing"
category = "landing"

[suite.py]
command = "uv run pytest"
parser = "junit"
level = "unit"
narrow_command = "uv run pytest -v {tests}"
narrow_tests = "pipeline/tests/*.py"
narrow_root = "pipeline/"
narrow_each = "$2"

[binding.unit]
when = ["test_step"]
TOML
}
mk plain.toml '{}'
mk file.toml '{file}'
out=$(cargo run -q -p mason-pipeline --bin mason -- definition check "$dir/.mason/plain.toml" 2>&1) || { echo "$out"; exit 1; }
echo "$out" | grep -q "pytest takes paths" || { echo "no warning for {}"; echo "$out"; exit 1; }
out=$(cargo run -q -p mason-pipeline --bin mason -- definition check "$dir/.mason/file.toml" 2>&1) || { echo "$out"; exit 1; }
if echo "$out" | grep -q "pytest takes paths"; then echo "warned for {file}"; exit 1; fi
echo "$out" | grep -q "^ok" || { echo "$out"; exit 1; }
echo "s871 passed"

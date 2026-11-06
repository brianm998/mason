#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Fails if a source file lacks an SPDX-License-Identifier header in its first
# 3 lines. With arguments, checks only those files; otherwise all tracked and
# untracked-but-not-ignored files. Files under templates/ must say MIT.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
if [ "$#" -gt 0 ]; then files=("$@"); else
  files=(); while IFS= read -r f; do files+=("$f"); done < <(git ls-files --cached --others --exclude-standard)
fi
bad=0
for f in "${files[@]}"; do
  [ -f "$f" ] || continue
  case "$f" in
    *.rs|*.sh|*.toml|*.yml|*.yaml|*.proto|*.swift|*.kt|*.kts|*.py) ;;
    *) continue ;;
  esac
  case "$f" in Cargo.lock|*/Cargo.lock) continue ;; esac
  want="GPL-3.0-or-later"
  case "$f" in templates/*) want="MIT" ;; esac
  if ! head -n 3 "$f" | grep -q "SPDX-License-Identifier: $want"; then
    echo "missing SPDX-License-Identifier: $want in $f" >&2; bad=1
  fi
done
exit $bad

#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1040: the portability lint uses no gawk-only `\x` escape, and refuses the same constructs under
# the awk on PATH and under mawk (ubuntu-latest's awk) when present.
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"
if grep -n '\\x[0-9A-Fa-f]' scripts/check-exit-portability.sh; then
  echo "FAIL check-exit-portability.sh uses a gawk-only \\x escape"; exit 1
fi
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
mkdir "$tmp/bin"
run_with() { # LABEL AWK
  local label=$1 awkbin=$2
  ln -sf "$awkbin" "$tmp/bin/awk"
  printf '#!/usr/bin/env bash\n# SPDX-License-Identifier: GPL-3.0-or-later\nsed -i %s x f\n' "''" > "$tmp/sed.sh"
  pc='(?'; printf '#!/usr/bin/env bash\n# SPDX-License-Identifier: GPL-3.0-or-later\necho a | grep -E %s\n' "'${pc}:a)'" > "$tmp/grep.sh"
  printf '#!/usr/bin/env bash\n# SPDX-License-Identifier: GPL-3.0-or-later\ncargo test -p x --test it a:: b::\n' > "$tmp/cargo.sh" # guard-ok: fixture
  printf '#!/usr/bin/env bash\n# SPDX-License-Identifier: GPL-3.0-or-later\necho ok\n' > "$tmp/clean.sh"
  for f in sed grep cargo; do
    if PATH="$tmp/bin:$PATH" bash scripts/check-exit-portability.sh "$tmp/$f.sh" >/dev/null 2>&1; then
      echo "FAIL ($label) $f fixture was not refused"; exit 1
    fi
  done
  PATH="$tmp/bin:$PATH" bash scripts/check-exit-portability.sh "$tmp/clean.sh" \
    || { echo "FAIL ($label) clean fixture refused"; exit 1; }
}
run_with awk "$(command -v awk)"
if command -v mawk >/dev/null 2>&1; then run_with mawk "$(command -v mawk)"; fi
echo "s1040: ok"

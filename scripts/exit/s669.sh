#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s669: the lint suite (as .mason/mason.toml defines it, minus its cargo parts) refuses a script
# without SPDX, a PCRE `grep -E` and two cargo test names; the changed-script selector picks scripts
# that name a changed path, and none for a docs-only change.
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
cd "$tmp"
git init -q .
git config user.email t@example.com; git config user.name t
mkdir -p scripts/exit .mason docs .github/workflows templates/github
# The exit-script guard reads the workspace's package names.
mkdir -p crates/x; printf '# SPDX-License-Identifier: GPL-3.0-or-later\n[package]\nname = "x"\n' > crates/x/Cargo.toml
# The lint parses workflow files; one of each kind so its globs match.
printf '# SPDX-License-Identifier: GPL-3.0-or-later\non: push\n' > .github/workflows/a.yml
printf '# SPDX-License-Identifier: MIT\non: push\n' > templates/github/a.yml
cp "$root"/scripts/check-spdx.sh "$root"/scripts/check-workflows.sh "$root"/scripts/check-exit-guards.sh "$root"/scripts/check-exit-portability.sh \
   "$root"/scripts/select-exit-scripts.sh "$root"/scripts/run-changed-exit-scripts.sh scripts/
cp "$root"/.mason/mason.toml .mason/
# The lint command, as defined, without the cargo steps (which need a workspace).
cmd=$(sed -n '/^\[suite.lint\]/,/^level/p' .mason/mason.toml | sed -n 's/^command = "\(.*\)"$/\1/p')
cmd=${cmd%% && cargo*}
case "$cmd" in *check-spdx.sh*check-exit-portability.sh*) ;; *) echo "FAIL lint does not run spdx+portability: $cmd"; exit 1 ;; esac
printf '#!/usr/bin/env bash\n# SPDX-License-Identifier: GPL-3.0-or-later\necho ok\n' > scripts/exit/s1.sh
printf '#!/usr/bin/env bash\n# SPDX-License-Identifier: GPL-3.0-or-later\nbash scripts/check-spdx.sh >/dev/null\n' > scripts/exit/s2.sh
echo docs > docs/a.md
git add -A; git commit -qm base
base=$(git rev-parse HEAD)
bash -c "$cmd" || { echo "FAIL clean tree refused"; exit 1; }

expect_refused() { # LABEL FILE CONTENT
  printf '%b' "$3" > "$2"
  if out=$(bash -c "$cmd" 2>&1); then echo "FAIL $1 was not refused"; exit 1; fi
  case "$out" in *"$2"*) ;; *) echo "FAIL $1: refusal does not name $2: $out"; exit 1 ;; esac
  rm -f "$2"
  echo "ok: $1 refused"
}
expect_refused "missing SPDX" scripts/exit/s3.sh '#!/usr/bin/env bash\necho hi\n'
# The fixtures are built from pieces so this script does not trip the lint it tests.
pcre='(?'; empty="''"
expect_refused "PCRE grep -E" scripts/exit/s4.sh "#!/usr/bin/env bash\n# SPDX-License-Identifier: GPL-3.0-or-later\necho a | grep -E '${pcre}:a)'\n"
expect_refused "two cargo test names" scripts/exit/s5.sh '#!/usr/bin/env bash\n# SPDX-License-Identifier: GPL-3.0-or-later\ncargo test -p x --test it a:: b::\n' # guard-ok: fixture package
expect_refused "sed -i empty suffix" scripts/exit/s6.sh "#!/usr/bin/env bash\n# SPDX-License-Identifier: GPL-3.0-or-later\nsed -i $empty x f\n"
bash -c "$cmd" || { echo "FAIL tree not clean again"; exit 1; }

# Selector: a change to check-spdx.sh picks s2; docs-only picks none.
echo '# edit' >> scripts/check-spdx.sh; git commit -qam spdx
sel=$(bash scripts/select-exit-scripts.sh "$base")
[ "$sel" = "scripts/exit/s2.sh" ] || { echo "FAIL selector picked: $sel"; exit 1; }
git reset -q --hard "$base"
echo more >> docs/a.md; git commit -qam docs
sel=$(bash scripts/select-exit-scripts.sh "$base")
[ -z "$sel" ] || { echo "FAIL docs-only picked: $sel"; exit 1; }
echo "s669 OK"

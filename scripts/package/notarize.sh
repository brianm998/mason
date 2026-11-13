#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Notarize and staple FILE (.pkg or .dmg). Usage: notarize.sh <file>
# Env: APPLE_API_KEY_PATH (the App Store Connect .p8 file), APPLE_API_KEY_ID, APPLE_API_ISSUER_ID.
# On failure the notary log is printed. Shared by the CLI .pkg and the monitor .dmg.
set -euo pipefail
die() { echo "notarize: $*" >&2; exit 1; }
[ $# -eq 1 ] || die "usage: notarize.sh <file>"
file=$1
[ -f "$file" ] || die "no such file: $file"
for v in APPLE_API_KEY_PATH APPLE_API_KEY_ID APPLE_API_ISSUER_ID; do
  [ -n "${!v:-}" ] || die "$v is not set"
done
[ -f "$APPLE_API_KEY_PATH" ] || die "APPLE_API_KEY_PATH is not a file"

res=$(mktemp "${TMPDIR:-/tmp}/mason-notary.XXXXXX")
trap 'rm -f "$res"' EXIT
set +e
xcrun notarytool submit "$file" --wait --output-format json \
  --key "$APPLE_API_KEY_PATH" --key-id "$APPLE_API_KEY_ID" --issuer "$APPLE_API_ISSUER_ID" >"$res"
rc=$?
set -e
cat "$res"
status=$(/usr/bin/plutil -extract status raw -o - "$res" 2>/dev/null || true)
if [ "$rc" -ne 0 ] || [ "$status" != Accepted ]; then
  id=$(/usr/bin/plutil -extract id raw -o - "$res" 2>/dev/null || true)
  if [ -n "$id" ]; then
    xcrun notarytool log "$id" --key "$APPLE_API_KEY_PATH" --key-id "$APPLE_API_KEY_ID" \
      --issuer "$APPLE_API_ISSUER_ID" || true
  fi
  die "notarization of $file failed (status '${status:-none}')"
fi
xcrun stapler staple "$file"

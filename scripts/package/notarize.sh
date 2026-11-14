#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Notarize FILE (.pkg, .dmg or .zip) with Apple and staple the ticket (not for .zip).
# Usage: notarize.sh <file>
# Credentials, first match wins:
#   APPLE_API_KEY_PATH (the App Store Connect .p8 file) + APPLE_API_KEY_ID + APPLE_API_ISSUER_ID
#   NOTARY_PROFILE (a `xcrun notarytool store-credentials` keychain profile)
#   APPLE_ID + APPLE_TEAM_ID + APPLE_APP_PASSWORD
# On failure the notary log is printed. Shared by the CLI .pkg and the monitor .dmg.
set -euo pipefail
die() { echo "notarize: $*" >&2; exit 1; }
[ $# -eq 1 ] || die "usage: notarize.sh <file>"
file=$1
[ -f "$file" ] || die "no such file: $file"

if [ -n "${APPLE_API_KEY_PATH:-}" ] || [ -n "${APPLE_API_KEY_ID:-}" ] || [ -n "${APPLE_API_ISSUER_ID:-}" ]; then
  for v in APPLE_API_KEY_PATH APPLE_API_KEY_ID APPLE_API_ISSUER_ID; do
    [ -n "${!v:-}" ] || die "$v is not set"
  done
  [ -f "$APPLE_API_KEY_PATH" ] || die "APPLE_API_KEY_PATH is not a file"
  auth=(--key "$APPLE_API_KEY_PATH" --key-id "$APPLE_API_KEY_ID" --issuer "$APPLE_API_ISSUER_ID")
elif [ -n "${NOTARY_PROFILE:-}" ]; then
  auth=(--keychain-profile "$NOTARY_PROFILE")
elif [ -n "${APPLE_ID:-}" ] && [ -n "${APPLE_TEAM_ID:-}" ] && [ -n "${APPLE_APP_PASSWORD:-}" ]; then
  auth=(--apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" --password "$APPLE_APP_PASSWORD")
else
  die "set APPLE_API_KEY_PATH/APPLE_API_KEY_ID/APPLE_API_ISSUER_ID, or NOTARY_PROFILE, or APPLE_ID, APPLE_TEAM_ID and APPLE_APP_PASSWORD"
fi

res=$(mktemp "${TMPDIR:-/tmp}/mason-notary.XXXXXX")
trap 'rm -f "$res"' EXIT
set +e
xcrun notarytool submit "$file" --wait --output-format json "${auth[@]}" >"$res"
rc=$?
set -e
cat "$res"
status=""
id=""
if [ "$rc" -eq 0 ]; then
  status=$(/usr/bin/plutil -extract status raw -o - "$res" 2>/dev/null) || status=""
fi
id=$(/usr/bin/plutil -extract id raw -o - "$res" 2>/dev/null) || id=""
if [ "$rc" -ne 0 ] || [ "$status" != Accepted ]; then
  if [ -n "$id" ]; then
    # Best effort: the log is for the reader; the failure below is what counts.
    if ! xcrun notarytool log "$id" "${auth[@]}"; then
      echo "notarize: could not fetch the notary log for $id" >&2
    fi
  fi
  die "notarization of $file failed (status '${status:-none}')"
fi
case "$file" in
  *.zip) ;;
  *) xcrun stapler staple "$file"; xcrun stapler validate "$file" ;;
esac

#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Notarize a signed .dmg, .pkg or .zip with Apple and staple the ticket (not for .zip).
# Usage: notarize.sh FILE
# Credentials, either: NOTARY_PROFILE (a `xcrun notarytool store-credentials` keychain profile),
# or APPLE_ID + APPLE_TEAM_ID + APPLE_APP_PASSWORD. Shared by the .pkg and .dmg packaging.
set -euo pipefail
file="${1:?usage: notarize.sh FILE}"
[ -f "$file" ] || { echo "notarize: no such file $file" >&2; exit 1; }
if [ -n "${NOTARY_PROFILE:-}" ]; then
  auth=(--keychain-profile "$NOTARY_PROFILE")
elif [ -n "${APPLE_ID:-}" ] && [ -n "${APPLE_TEAM_ID:-}" ] && [ -n "${APPLE_APP_PASSWORD:-}" ]; then
  auth=(--apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" --password "$APPLE_APP_PASSWORD")
else
  echo "notarize: set NOTARY_PROFILE, or APPLE_ID, APPLE_TEAM_ID and APPLE_APP_PASSWORD" >&2; exit 1
fi
xcrun notarytool submit "$file" "${auth[@]}" --wait
case "$file" in
  *.zip) ;;
  *) xcrun stapler staple "$file"; xcrun stapler validate "$file" ;;
esac

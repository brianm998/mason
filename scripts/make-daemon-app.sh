#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Wraps a masond launcher in "MASON Daemon.app", a background app (no Dock icon, no menu bar),
# and signs it, so macOS has a proper app to grant permissions to.
#
#   scripts/make-daemon-app.sh MASOND DEST_DIR [IDENTITY]
#
# MASOND is the launcher the launch agent runs today (it execs runtimes/current/masond; it finds
# the runtime from --data-dir, not from where it sits). DEST_DIR gets "MASON Daemon.app".
# IDENTITY is a code-signing certificate's SHA-1 or name, e.g. a Developer ID Application
# identity; without one the app is signed ad hoc.
#
# Why an app: macOS runs a malware scan on every launch of non-Apple code that has no cached
# result, and a pipeline launches a great deal of freshly built code. Processes started by an
# app listed under System Settings > Privacy & Security > Developer Tools skip those scans. The
# pane takes apps, and with a real identity the grant follows the team ID and bundle identifier,
# so it survives rebuilding the app and every self-update of the runtime behind it.
#
# Afterwards point the launch agent's ProgramArguments[0] at
#   DEST_DIR/MASON Daemon.app/Contents/MacOS/masond
# restart it, and add "MASON Daemon.app" under Developer Tools.
set -euo pipefail

die() { echo "make-daemon-app: $*" >&2; exit 1; }
[ $# -ge 2 ] && [ $# -le 3 ] || die "usage: $0 MASOND DEST_DIR [IDENTITY]"
src=$1; dest=$2; identity=${3:-}
[ -f "$src" ] || die "no launcher at $src"
[ -d "$dest" ] || die "no directory $dest"
[ "$(uname -s)" = Darwin ] || die "macOS only"

app="$dest/MASON Daemon.app"
work="$dest/.MASON Daemon.app.partial"
rm -rf "$work"
mkdir -p "$work/Contents/MacOS"
cp "$src" "$work/Contents/MacOS/masond"
chmod 755 "$work/Contents/MacOS/masond"
cat > "$work/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key><string>com.brianm998.mason.daemon</string>
  <key>CFBundleName</key><string>MASON Daemon</string>
  <key>CFBundleDisplayName</key><string>MASON Daemon</string>
  <key>CFBundleExecutable</key><string>masond</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>1.0</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>LSUIElement</key><true/>
  <key>LSBackgroundOnly</key><true/>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
</dict>
</plist>
PLIST

if [ -n "$identity" ]; then
  /usr/bin/codesign --sign "$identity" --options runtime --force \
    --identifier com.brianm998.mason.daemon "$work"
else
  /usr/bin/codesign --sign - --force --identifier com.brianm998.mason.daemon "$work"
fi
/usr/bin/codesign --verify --strict "$work" || die "the signed app does not verify"

# Replace any earlier app only once the new one verifies. The old one is moved aside, not
# deleted, so a launch agent still running it keeps a valid binary.
if [ -e "$app" ]; then
  mv "$app" "$dest/.MASON Daemon.app.old-$(date +%Y%m%d%H%M%S)"
fi
mv "$work" "$app"
echo "made $app"
/usr/bin/codesign -dv "$app" 2>&1 | grep -E '^(Identifier|TeamIdentifier|Signature|Runtime)' || true
echo "launch agent program: $app/Contents/MacOS/masond"

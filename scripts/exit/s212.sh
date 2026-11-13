#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s212 exit test: a daemon restart's lost hosts are not counted against the task, and (on macOS)
# a process detached with setsid is tested against `launchctl kickstart -k` of a TEST launch
# agent, with and without AbandonProcessGroup. Never touches the owner's launch agent.
set -uo pipefail
cd "$(dirname "$0")/../.."
if [ -z "${S212_SKIP_CARGO:-}" ]; then
  cargo test -q -p mason-daemon --lib hostloss || exit 1
fi
if [ "$(uname)" != "Darwin" ]; then
  echo "s212 (d): SKIPPED, not macOS (no launchd)"; exit 0
fi
uid=$(id -u); dir=$(mktemp -d /tmp/s212.XXXXXX)
label="com.mason.test.s212.$$"
trap 'launchctl bootout "gui/$uid/$label" 2>/dev/null; [ -f "$dir/pid" ] && kill "$(cat "$dir/pid")" 2>/dev/null; rm -rf "$dir"' EXIT
cat > "$dir/run.sh" <<SH
#!/bin/bash
# a "step host": its own session, as host.rs does with setsid
/usr/bin/perl -MPOSIX -e 'POSIX::setsid(); exec "/bin/sleep", "300"' &
echo \$! > "$dir/pid.new"; mv "$dir/pid.new" "$dir/pid"
exec /bin/sleep 300
SH
chmod +x "$dir/run.sh"
try() { # $1 = AbandonProcessGroup value, $2 = how the job is stopped: kickstart or stop
  rm -f "$dir/pid"
  cat > "$dir/$label.plist" <<PL
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>$label</string>
<key>ProgramArguments</key><array><string>/bin/bash</string><string>$dir/run.sh</string></array>
<key>RunAtLoad</key><true/>
<key>AbandonProcessGroup</key><$1/>
</dict></plist>
PL
  launchctl bootstrap "gui/$uid" "$dir/$label.plist" || { echo "s212 (d): SKIPPED, bootstrap failed"; exit 0; }
  for _ in $(seq 1 50); do [ -f "$dir/pid" ] && break; sleep 0.1; done
  pid=$(cat "$dir/pid" 2>/dev/null) || { echo "s212 (d): SKIPPED, no pid"; exit 0; }
  if [ "$2" = stop ]; then launchctl stop "$label"; else launchctl kickstart -k "gui/$uid/$label"; fi; sleep 2
  if kill -0 "$pid" 2>/dev/null; then r=survived; kill "$pid"; else r=killed; fi
  launchctl bootout "gui/$uid/$label" 2>/dev/null
  echo "s212 (d): AbandonProcessGroup=$1, $2: detached child $r"
  [ "$r" = survived ]
}
try false kickstart || true
try false stop || true
try true stop || { echo "s212 (d): FAIL with AbandonProcessGroup=true (stop)"; exit 1; }
try true kickstart || { echo "s212 (d): FAIL with AbandonProcessGroup=true"; exit 1; }
echo "s212 (d): ran"

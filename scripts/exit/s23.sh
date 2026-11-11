#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 23 exit test: notifications, sound and speech. A real masond; fake SMTP, ntfy and webhook
# receivers and fake monitors on loopback; a fake speech command. No real service, device or sound.
#  1. A condition (a pause) produces one alert and one delivery on every routed channel (email,
#     ntfy, webhook, and one announcement), one reminder after the delay, and silence after it
#     clears. The webhook's signature verifies; the email does not carry the password.
#  2. Acknowledging stops the reminder; the alert stays listed; clearing and returning is a new one.
#  3. Two simultaneous announcements reach exactly one of two connected monitors, in order, one
#     at a time (no overlap in the fake sink's timestamps). A muted monitor never speaks.
#  4. When the speaker stops claiming for 30 s the other takes over.
#  5. The daemon speaks only with speech turned on and no monitor claiming, in order, through its
#     configured command.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s23.XXXXXX); daemon=""; pids=()
cleanup() {
  [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true
  for p in "${pids[@]:-}"; do [ -n "$p" ] && kill -9 "$p" 2>/dev/null || true; done
  rm -rf "$root"
}
trap cleanup EXIT
cargo build -q --workspace --bins
bin=target/debug
fakes="$PWD/scripts/exit/fixtures/s23/fakes.py"
owner=${USER:-owner}
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
fail() { echo "FAIL: $*" >&2; tail -20 "$root/masond.log" >&2 || true; exit 1; }
count() { [ -e "$1" ] && wc -l < "$1" | tr -d ' ' || echo 0; }
wait_for() {  # SECONDS WHAT CMD...
  local end=$((SECONDS + $1)) what=$2; shift 2
  until "$@" >/dev/null 2>&1; do [ $SECONDS -lt $end ] || fail "timed out waiting for $what"; sleep 0.2; done
}
has() { [ "$(count "$1")" -ge "$2" ]; }
nalerts() { m alerts --json | python3 -c 'import json,sys;print(len(json.load(sys.stdin)["alerts"]))'; }
cleared() { [ "$(nalerts)" = 0 ]; }
nannounced() { m notify --json | python3 -c 'import json,sys;print(len(json.load(sys.stdin)["recent"]))'; }
eq() { [ "$2" = "$3" ] || fail "$1: expected $3, got $2"; }

mkdir -p "$root/c" "$root/f"
python3 -I "$fakes" servers "$root/f" & pids+=($!)
wait_for 10 "the fake servers" test -e "$root/f/ports.json"
http=$(python3 -c "import json;print(json.load(open('$root/f/ports.json'))['http'])")
smtp=$(python3 -c "import json;print(json.load(open('$root/f/ports.json'))['smtp'])")
say="$root/say.sh"
cat > "$say" <<SH
#!/bin/sh
# the fake speech command: records when each sentence started and ended (it takes a second to "say" it)
s=\$(python3 -c 'import time;print(time.time())'); sleep 1; e=\$(python3 -c 'import time;print(time.time())')
printf '%s\t%s\t%s\n' "\$s" "\$e" "\$1" >> "$root/spoken.tsv"
SH
chmod +x "$say"
write_config() {  # DAEMON_SPEECH
  cat > "$root/c/notify.toml" <<TOML
# "paused" is a cheap condition to raise and clear; the table is overridden so it uses every channel.
[routing.paused]
sound = "0"
speech = "0"
push = "0"
email = "0"
webhook = "0"
[reminder]
after = "4s"
[speech]
daemon = $1
command = ["$say"]
[ntfy]
server = "http://127.0.0.1:$http"
topic = "mason"
[email]
server = "127.0.0.1:$smtp"
from = "mason@example.test"
to = "owner@example.test"
user = "mason"
password = "hunter2-smtp-password"
tls = "none"
[webhook]
url = "http://127.0.0.1:$http/hook"
secret = "hunter2-hook-secret"
TOML
}
write_config false

export MASON_TEST_MODE=1
"$bin/masond" --data-dir "$root/d" --config-dir "$root/c" --port 0 --heartbeat-ms 100 --sweep-ms 3600000 \
  --reload-ms 300 --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
wait_for 20 "masond" m status
port=$(python3 -c "import json,glob;print(json.load(open(glob.glob('$root/d/daemon.json')[0]))['port'])")

echo "== the defaults are Part 19's table"
tab=$(m notify)
echo "$tab" | grep -q "needs_you .*monitor, sound, speech, push after 10 min, email after 60 min" || fail "needs_you row: $tab"
echo "$tab" | grep -q "failed .*monitor, sound, speech, push after 10 min$" || fail "failed row: $tab"
echo "$tab" | grep -q "daemon_down .*monitor, sound, speech, push, email, webhook" || fail "daemon_down row: $tab"
echo "$tab" | grep -q "landed .*monitor, speech" || fail "landed row: $tab"

echo "== 1. one alert, one delivery per channel, one reminder, silence after it clears"
m pause --mode all --for 1h >/dev/null
wait_for 10 "the first deliveries" has "$root/f/webhook.jsonl" 1
wait_for 10 "the first push" has "$root/f/ntfy.jsonl" 1
wait_for 10 "the first mail" has "$root/f/mail.jsonl" 1
sleep 2
eq "alerts" "$(nalerts)" 1
eq "webhooks" "$(count "$root/f/webhook.jsonl")" 1
eq "ntfy" "$(count "$root/f/ntfy.jsonl")" 1
eq "mails" "$(count "$root/f/mail.jsonl")" 1
eq "announcements" "$(nannounced)" 1
python3 - "$root" <<'PY' || fail "webhook body or signature"
import json, sys
root = sys.argv[1]
r = json.loads(open(root + "/f/webhook.jsonl").readline())
assert r["headers"].get("X-Mason-Signature", "").startswith("sha256=")
body = json.loads(r["body"])
assert body["kind"] == "paused" and body["reminder"] is False, body
PY
sig=$(python3 -c "import json;print(json.loads(open('$root/f/webhook.jsonl').readline())['headers']['X-Mason-Signature'])")
body=$(python3 -c "import json;print(json.loads(open('$root/f/webhook.jsonl').readline())['body'],end='')")
python3 -I "$fakes" verify "hunter2-hook-secret" "$body" "$sig" || fail "the webhook signature does not verify"
! python3 -I "$fakes" verify "wrong-secret" "$body" "$sig" || fail "a wrong secret verified"
grep -q "owner@example.test" "$root/f/mail.jsonl" || fail "mail recipient"
! grep -q "hunter2" "$root/f/mail.jsonl" "$root/f/ntfy.jsonl" "$root/masond.log" || fail "a secret leaked into a message or the log"
grep -q "Title" "$root/f/ntfy.jsonl" || fail "ntfy title header"
# the reminder, once
wait_for 15 "the reminder" has "$root/f/webhook.jsonl" 2
sleep 6
eq "webhooks after the reminder" "$(count "$root/f/webhook.jsonl")" 2
eq "ntfy after the reminder" "$(count "$root/f/ntfy.jsonl")" 2
eq "mails after the reminder" "$(count "$root/f/mail.jsonl")" 2
# Queuing an announcement is not delivering it: with no monitor claiming yet and daemon speech
# off, nothing plays the first one, so the reminder adds none for sound and speech.
eq "announcements after the reminder" "$(nannounced)" 1
python3 -c "import json;r=[json.loads(l) for l in open('$root/f/webhook.jsonl')];assert [json.loads(x['body'])['reminder'] for x in r]==[False,True]" || fail "second webhook is not the reminder"
# clears: silence
m resume --mode all >/dev/null
wait_for 10 "the alert to clear" cleared
sleep 6
eq "webhooks after clearing" "$(count "$root/f/webhook.jsonl")" 2
eq "mails after clearing" "$(count "$root/f/mail.jsonl")" 2

echo "== 2. acknowledging stops the noise; a return is a new alert"
m pause --mode all --for 1h >/dev/null
wait_for 10 "the new alert's webhook" has "$root/f/webhook.jsonl" 3
m alerts ack "paused::all" >/dev/null
sleep 7
eq "webhooks after ack" "$(count "$root/f/webhook.jsonl")" 3
m alerts | grep -q "(acknowledged)" || fail "an acknowledged alert is still listed"
if python3 - "$port" <<'PY'
import json, sys, urllib.request
r = urllib.request.Request("http://127.0.0.1:%s/alerts/paused::all/ack" % sys.argv[1], data=json.dumps({"as": "somebody-else"}).encode(), headers={"Content-Type": "application/json"})
try:
    urllib.request.urlopen(r)
except urllib.error.HTTPError as e:
    sys.exit(0 if e.code == 403 else 2)
sys.exit(1)
PY
then :; else fail "a non-owner could acknowledge"; fi
m resume --mode all >/dev/null
wait_for 10 "the alert to clear" cleared
m pause --mode all --for 1h >/dev/null
wait_for 10 "a returning alert to be new" has "$root/f/webhook.jsonl" 4
m resume --mode all >/dev/null
wait_for 10 "clear" cleared

echo "== 3. two announcements, two monitors: one speaks, in order, one at a time; muted never speaks"
python3 -I "$fakes" client "$port" monitorA "$root/a.jsonl" & pids+=($!); pa=$!
python3 -I "$fakes" client "$port" monitorB "$root/b.jsonl" & pids+=($!); pb=$!
python3 -I "$fakes" client "$port" monitorM "$root/m.jsonl" muted & pids+=($!)
wait_for 10 "the monitors to claim" has "$root/a.jsonl.claims" 1
wait_for 10 "the monitors to claim" has "$root/b.jsonl.claims" 1
wait_for 10 "the monitors to claim" has "$root/m.jsonl.claims" 1
sleep 2
# Two conditions at once: two alerts, two announcements.
m pause --mode all --for 1h >/dev/null
m pause --mode claims --for 1h >/dev/null
wait_for 15 "both announcements to be played" bash -c "cat '$root'/a.jsonl '$root'/b.jsonl 2>/dev/null | grep -c '\"what\": \"say\"' | grep -qx 2"
python3 - "$root" <<'PY' || fail "announcements overlapped or went to both"
import json, sys
root = sys.argv[1]
def rows(n):
    try:
        return [json.loads(l) for l in open(f"{root}/{n}.jsonl")]
    except FileNotFoundError:
        return []
a, b, m = rows("a"), rows("b"), rows("m")
assert not m, "the muted monitor spoke"
speakers = [n for n, r in (("a", a), ("b", b)) if r]
assert len(speakers) == 1, f"exactly one monitor speaks, got {speakers}"
r = a or b
assert [x["what"] for x in r] == ["sound", "say", "sound", "say"], r
for prev, nxt in zip(r, r[1:]):
    assert nxt["start"] >= prev["end"] - 0.001, f"overlap: {prev} {nxt}"
assert r[0]["text"] == r[2]["text"], "both are the same family of sound"
print("speaker:", speakers[0], [x["key"] for x in r if x["what"] == "say"])
PY
# Muting leaves the visual alerts: the alerts are still listed while only the muted monitor is left.
eq "visible alerts" "$(nalerts)" 2
if [ -s "$root/a.jsonl" ]; then speaker=$pa; other=b; else speaker=$pb; other=a; fi
kill -9 "$speaker"
echo "== 4. the speaker stops claiming; after 30 s the other takes over"
sleep 1
m resume --mode all >/dev/null; m resume --mode claims >/dev/null
wait_for 10 "clear" cleared
sleep 31
before=$(count "$root/$other.jsonl")
m pause --mode landing --for 1h >/dev/null
wait_for 15 "the other monitor to take over" has "$root/$other.jsonl" $((before + 1))
m resume --mode landing >/dev/null
last=$(tail -1 "$root/$other.jsonl.claims")
echo "$last" | grep -q '"speaker": true' || fail "the survivor is not the speaker: $last"

echo "== 5. daemon speech: only with it on and no monitor claiming; in order, one at a time"
write_config true
sleep 1
m notify test speech >/dev/null; m notify test speech >/dev/null
sleep 4
[ ! -e "$root/spoken.tsv" ] || fail "the daemon spoke while a monitor was claiming"
kill -9 "${pids[@]:1}" 2>/dev/null || true
pids=("${pids[0]}")
sleep 33
m notify test speech >/dev/null; m notify test speech >/dev/null
wait_for 20 "the daemon to speak" bash -c "test \$(wc -l < '$root/spoken.tsv') -ge 2"
python3 - "$root" <<'PY' || fail "daemon speech overlapped"
import sys
rows = [l.rstrip("\n").split("\t") for l in open(sys.argv[1] + "/spoken.tsv")]
assert len(rows) >= 2
for p, n in zip(rows, rows[1:]):
    assert float(n[0]) >= float(p[1]) - 0.001, f"overlap {p} {n}"
print("daemon spoke", len(rows), "times without overlap")
PY
echo "s23 OK"

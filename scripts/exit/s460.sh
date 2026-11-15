#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s460 exit test: the APNs push channel. A real masond with the LAN listener on and [apns] pointing
# at a fake APNs on loopback (the endpoint override), a fake phone that pairs and registers a push
# token, and a project whose agent asks a question (a needs-attention condition).
#  1. An unpaired caller cannot register a token; the paired phone can, for itself only.
#  2. The question raises one alert and exactly one POST reaches the fake APNs, to the phone's
#     token, with a JWT that verifies against the owner's key and the task id in the payload.
#  3. `mason notify test apns` sends one more to every eligible device.
#  4. A key file others can read is refused, and nothing is sent.
#  5. Revoking the phone clears its token: a further test has no device to send to.
# A real push to a phone is the owner's check (docs/stages/s460.md), not run here.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
command -v python3 >/dev/null || { echo "s460 needs python3 (its fake phone and APNs)" >&2; exit 1; }
command -v openssl >/dev/null || { echo "s460 needs openssl (to make the key)" >&2; exit 1; }
root=$(mktemp -d /tmp/mason-s460.XXXXXX); daemon=""; fake=""
cleanup() {
  [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true
  [ -n "$fake" ] && kill -9 "$fake" 2>/dev/null || true
  for f in "$root"/d/h/*/host.kv; do
    [ -e "$f" ] || continue
    hp=$(sed -n "s/^host_pid=//p" "$f"); cp=$(sed -n "s/^child_pid=//p" "$f")
    if [ -n "$hp" ] && ps -o command= -p "$hp" 2>/dev/null | grep -qF "step-host --dir $(dirname "$f")"; then kill -9 "-$cp" 2>/dev/null || true; kill -9 "$hp" 2>/dev/null || true; fi
  done
  rm -rf "$root"
}
trap cleanup EXIT
cargo build -q --workspace --bins
bin=target/debug
owner=${USER:-owner}
free() { python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])'; }
port=$(free); lanport=$(free); apnsport=$(free)
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
fail() { echo "FAIL: $*" >&2; tail -20 "$root/masond.log" >&2 || true; exit 1; }
wait_for() {  # SECONDS WHAT CMD...
  local end=$((SECONDS + $1)) what=$2; shift 2
  until "$@" >/dev/null 2>&1; do [ $SECONDS -lt $end ] || fail "timed out waiting for $what"; sleep 0.2; done
}
posts() { [ -e "$root/apns.jsonl" ] && wc -l < "$root/apns.jsonl" | tr -d ' ' || echo 0; }
mkdir -p "$root/c/projects"

# The owner's key: a P-256 key in PKCS#8 PEM, as Apple's .p8 is, mode 0600.
openssl ecparam -name prime256v1 -genkey -noout 2>/dev/null | openssl pkcs8 -topk8 -nocrypt -out "$root/c/AuthKey_TEST.p8" 2>/dev/null
chmod 600 "$root/c/AuthKey_TEST.p8"
openssl pkey -in "$root/c/AuthKey_TEST.p8" -pubout -out "$root/pub.pem" 2>/dev/null
printf '[apns]\nkey_file = "%s/c/AuthKey_TEST.p8"\nkey_id = "KEYID12345"\nteam_id = "TEAMID1234"\ntopic = "com.example.mason"\nendpoint = "http://127.0.0.1:%s"\n' "$root" "$apnsport" > "$root/c/notify.toml"
printf '[lan]\nenabled = true\nport = %s\n' "$lanport" > "$root/c/daemon.toml"

python3 -I scripts/exit/fixtures/s460/fake_apns.py "$apnsport" "$root/apns.jsonl" & fake=$!

# A project whose agent asks a question.
repo="$root/askshop"
git init -q --bare -b main "$root/origin.git"
git clone -q "$root/origin.git" "$repo" 2>/dev/null
git -C "$repo" checkout -q -b main
mkdir -p "$repo/.mason"
cat > "$repo/.mason/mason.toml" <<'TOML'
schema = 1
start = "explore"
explored_entry = "develop"

[step.explore]
type = "agent"
category = "exploring"
next = "develop"

[step.develop]
type = "agent"
category = "developing"
next = "test"

[step.test]
type = "test"
category = "testing"
max_rounds = 2
on_red = "develop"
next = "land"

[step.land]
type = "landing"
category = "landing"
TOML
printf 'mode = "direct"\n' > "$repo/.mason/landing.toml"
echo a > "$repo/a.txt"
git -C "$repo" add -A; git -C "$repo" -c user.name=t -c user.email=t@t commit -qm first
git -C "$repo" push -q origin main
cat > "$root/ask.json" <<'JSON'
{"steps": {"develop": [[
  {"call": {"tool": "ask", "args": {"ask": "Should the token expire after one hour or one day?", "options": [{"label": "one hour", "recommended": true}, {"label": "one day"}]}}}
]]}}
JSON
cat > "$root/c/projects/askshop.toml" <<TOML
path = "$repo"
base = "main"
remote = "origin"
test = "true"
claude_bin = "$PWD/$bin/fake-claude"
[agent_env]
FAKE_CLAUDE_SCRIPT = "$root/ask.json"
TOML

"$bin/masond" --data-dir "$root/d" --config-dir "$root/c" --port "$port" --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" 2>/dev/null || fail "masond did not start"; sleep 0.1; done

echo "== 1. the phone pairs and registers a push token for itself"
code=$(m pair --json | python3 -c 'import json,sys;print(json.load(sys.stdin)["code"])')
out=$(python3 -I scripts/exit/fixtures/s460/phone.py "$lanport" "$code") || fail "the fake phone failed: $out"
token=$(printf '%s\n' "$out" | sed -n 's/^TOKEN //p')
[ "$(posts)" = 0 ] || fail "a push went out before anything happened"
echo "PASS: an unpaired caller is refused; the paired phone registered, a bad token was refused"

echo "== 2. a question reaches the phone"
m task add "choose the token lifetime" --project askshop --explored --body "x" >/dev/null
has_push() { [ "$(posts)" -ge 1 ]; }
wait_for 90 "one push" has_push
sleep 2
[ "$(posts)" = 1 ] || fail "expected exactly one POST, got $(posts)"
python3 -I - "$root/apns.jsonl" "$root/pub.pem" "$root" <<'PY' || fail "the POST is not what APNs wants"
import base64, json, subprocess, sys
rec = json.loads(open(sys.argv[1]).readline())
assert rec["path"] == "/3/device/0a1b2c3d4e5f", rec["path"]
h = rec["headers"]
assert h["apns-topic"] == "com.example.mason" and h["apns-push-type"] == "alert", h
assert h["authorization"].lower().startswith("bearer "), h
jwt = h["authorization"].split(" ", 1)[1]
a, b, c = jwt.split(".")
d = lambda s: base64.urlsafe_b64decode(s + "=" * (-len(s) % 4))
assert json.loads(d(a)) == {"alg": "ES256", "kid": "KEYID12345"}
assert json.loads(d(b))["iss"] == "TEAMID1234"
# The signature is raw r||s; openssl wants DER.
sig = d(c); assert len(sig) == 64
def der(x):
    x = x.lstrip(b"\0") or b"\0"
    x = (b"\0" + x) if x[0] & 0x80 else x
    return b"\x02" + bytes([len(x)]) + x
body = der(sig[:32]) + der(sig[32:])
sigder = b"\x30" + bytes([len(body)]) + body
root = sys.argv[3]
open(root + "/sig.der", "wb").write(sigder)
open(root + "/signed.txt", "wb").write((a + "." + b).encode())
r = subprocess.run(["openssl", "dgst", "-sha256", "-verify", sys.argv[2], "-signature", root + "/sig.der", root + "/signed.txt"], capture_output=True, text=True)
assert r.returncode == 0, r.stdout + r.stderr
p = json.loads(rec["body"])
assert p["mason"]["kind"] == "needs_you" and isinstance(p["mason"]["task"], int), p
assert p["aps"]["alert"]["title"].startswith("MASON"), p
assert p["aps"]["thread-id"] == "askshop", p
print("ok")
PY
if grep -q "Bearer\|authorization" "$root/masond.log"; then fail "the JWT is in masond.log"; fi
echo "PASS: one POST, a valid JWT, the task id and project in the payload"

echo "== 3. mason notify test apns"
m notify test apns >/dev/null || fail "notify test apns failed"
[ "$(posts)" = 2 ] || fail "expected the test push, got $(posts) POSTs"
echo "PASS: the test push went to the device"

echo "== 4. a readable key file is refused"
chmod 644 "$root/c/AuthKey_TEST.p8"
if m notify test apns >/dev/null 2>"$root/err.txt"; then fail "a world-readable key was used"; fi
grep -q 0600 "$root/err.txt" || fail "the refusal did not say 0600: $(cat "$root/err.txt")"
[ "$(posts)" = 2 ] || fail "something was sent with a readable key"
chmod 600 "$root/c/AuthKey_TEST.p8"
echo "PASS: refused, nothing sent"

echo "== 5. revoking the phone clears its token"
id=$(m devices list --json | python3 -c 'import json,sys;print(json.load(sys.stdin)["devices"][0]["id"])')
m devices revoke "$id" >/dev/null
if m notify test apns >/dev/null 2>"$root/err.txt"; then fail "a push went to a revoked device"; fi
[ "$(posts)" = 2 ] || fail "a revoked device was pushed to"
echo "PASS: no device left to send to"
echo "s460: all checks passed"

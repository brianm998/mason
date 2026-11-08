#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 23, for real: one test message through your own SMTP server, your ntfy topic, your
# webhook and your machine's speech command. MANUAL: it sends real email and real push, so it is
# never run by CI and never by an agent on its own. Nothing here is asserted beyond "the daemon
# reported the send as accepted"; you check your inbox, phone and speakers.
#
# Needs, all explicit:
#   MASON_REAL_APPROVED=yes      you have read this script and approve running it
# and any of these (a channel with no settings is skipped):
#   MASON_REAL_SMTP=host:587     MASON_REAL_SMTP_FROM, MASON_REAL_SMTP_TO, and optionally
#                                MASON_REAL_SMTP_USER and MASON_REAL_SMTP_PASSWORD_FILE
#                                (STARTTLS is required; the password never goes on a command line)
#   MASON_REAL_NTFY_TOPIC=name   optionally MASON_REAL_NTFY_SERVER (default https://ntfy.sh)
#   MASON_REAL_WEBHOOK_URL=url   optionally MASON_REAL_WEBHOOK_SECRET_FILE
#   MASON_REAL_SPEECH=say        a program that speaks its last argument (say, espeak-ng, spd-say)
set -euo pipefail
[ "${MASON_REAL_APPROVED:-}" = yes ] || { echo "refusing: set MASON_REAL_APPROVED=yes once you approve running this" >&2; exit 2; }
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s23r.XXXXXX); daemon=""
cleanup() { [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true; rm -rf "$root"; }
trap cleanup EXIT
cargo build -q --workspace --bins
bin=target/debug
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
mkdir -p "$root/c"
{
  if [ -n "${MASON_REAL_SMTP:-}" ]; then
    echo "[email]"; echo "server = \"$MASON_REAL_SMTP\""; echo "from = \"${MASON_REAL_SMTP_FROM:?set MASON_REAL_SMTP_FROM}\""
    echo "to = \"${MASON_REAL_SMTP_TO:?set MASON_REAL_SMTP_TO}\""
    [ -z "${MASON_REAL_SMTP_USER:-}" ] || echo "user = \"$MASON_REAL_SMTP_USER\""
    [ -z "${MASON_REAL_SMTP_PASSWORD_FILE:-}" ] || echo "password_file = \"$MASON_REAL_SMTP_PASSWORD_FILE\""
  fi
  if [ -n "${MASON_REAL_NTFY_TOPIC:-}" ]; then
    echo "[ntfy]"; echo "server = \"${MASON_REAL_NTFY_SERVER:-https://ntfy.sh}\""; echo "topic = \"$MASON_REAL_NTFY_TOPIC\""
  fi
  if [ -n "${MASON_REAL_WEBHOOK_URL:-}" ]; then
    echo "[webhook]"; echo "url = \"$MASON_REAL_WEBHOOK_URL\""
    [ -z "${MASON_REAL_WEBHOOK_SECRET_FILE:-}" ] || echo "secret_file = \"$MASON_REAL_WEBHOOK_SECRET_FILE\""
  fi
  if [ -n "${MASON_REAL_SPEECH:-}" ]; then
    echo "[speech]"; echo "daemon = true"; echo "command = [\"$MASON_REAL_SPEECH\"]"
  fi
} > "$root/c/notify.toml"
"$bin/masond" --data-dir "$root/d" --config-dir "$root/c" --port 0 --reload-ms 500 2>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.2; done
ran=0
for ch in email push webhook; do
  case $ch in
    email) [ -n "${MASON_REAL_SMTP:-}" ] || continue ;;
    push) [ -n "${MASON_REAL_NTFY_TOPIC:-}" ] || continue ;;
    webhook) [ -n "${MASON_REAL_WEBHOOK_URL:-}" ] || continue ;;
  esac
  echo "== $ch"; m notify test "$ch"; ran=1
done
if [ -n "${MASON_REAL_SPEECH:-}" ]; then
  echo "== speech (the daemon speaks only because no monitor is open)"; m notify test speech; ran=1
  sleep 15
fi
[ "$ran" = 1 ] || { echo "nothing to test: set at least one of the MASON_REAL_* settings above" >&2; exit 2; }
echo "Now check: the email arrived (and its headers show no password), the phone got the push, the webhook receiver verified its X-Mason-Signature, and the sentence was spoken once."

#!/bin/bash
# SPDX-License-Identifier: GPL-3.0-or-later
# A watchdog that lives outside masond: say when the daemon is dead, hung or crash-looping.
# Run once a minute by a launchd agent (docs/examples/com.example.mason-watchdog.plist).
# Only Apple's own tools are used (/bin/bash, /usr/bin/curl, /usr/bin/say, /bin/date) because
# third-party binaries freeze at launch while syspolicyd is down, and this must still run then.
#
# Each run asks GET /status (which goes through the worker pool and the store's writer, so a
# wedged daemon does not answer) with a 20-second limit.
#   two failures in a row            -> "down"; repeated every 30 minutes; "back" said once
#   pid changed or uptime_s < 60 on three runs in a row -> "crash-looping"
#
# Environment (all optional; the defaults are the real ones):
#   MASON_DATA_DIR            data directory (default ~/Library/Application Support/mason)
#   MASON_CONFIG_DIR          config directory holding notify.toml (default ~/.config/mason)
#   MASON_WATCHDOG_DAEMON_JSON  path of daemon.json
#   MASON_WATCHDOG_STATE      state file (default: mason-watchdog.state beside the data directory)
#   MASON_WATCHDOG_NOTIFY_TOML  path of notify.toml
#   MASON_WATCHDOG_SAY        the speech command (default /usr/bin/say; skipped when absent)
#   MASON_WATCHDOG_CURL       curl (default /usr/bin/curl)
#   MASON_WATCHDOG_TIMEOUT    seconds to wait for /status (default 20)
#   MASON_WATCHDOG_NOW        epoch seconds standing in for the clock (tests)
set -u

case "$(uname -s)" in
Darwin) default_data="$HOME/Library/Application Support/mason" ;;
*) default_data="${XDG_DATA_HOME:-$HOME/.local/share}/mason" ;;
esac
data_dir=${MASON_DATA_DIR:-$default_data}
config_dir=${MASON_CONFIG_DIR:-$HOME/.config/mason}
djson=${MASON_WATCHDOG_DAEMON_JSON:-$data_dir/daemon.json}
state=${MASON_WATCHDOG_STATE:-$(dirname "$data_dir")/mason-watchdog.state}
ntoml=${MASON_WATCHDOG_NOTIFY_TOML:-$config_dir/notify.toml}
say_cmd=${MASON_WATCHDOG_SAY:-/usr/bin/say}
curl_cmd=${MASON_WATCHDOG_CURL:-/usr/bin/curl}
timeout=${MASON_WATCHDOG_TIMEOUT:-20}
now=${MASON_WATCHDOG_NOW:-$(/bin/date +%s)}
repeat_s=1800

# ---- state (key=value lines; read, never sourced) ------------------------------------
fails=0 mode=up last_pid="" churn=0 last_alert=0 crash_alerted=0 last_up="" last_now=0
if [ -r "$state" ]; then
  while IFS='=' read -r k v; do
    case "$k" in
    fails) fails=$v ;; mode) mode=$v ;; last_pid) last_pid=$v ;; churn) churn=$v ;;
    last_alert) last_alert=$v ;; crash_alerted) crash_alerted=$v ;;
    last_up) last_up=$v ;; last_now) last_now=$v ;;
    esac
  done <"$state"
fi
case "$fails" in '' | *[!0-9]*) fails=0 ;; esac
case "$churn" in '' | *[!0-9]*) churn=0 ;; esac
case "$last_alert" in '' | *[!0-9]*) last_alert=0 ;; esac
case "$last_now" in '' | *[!0-9]*) last_now=0 ;; esac
case "$last_up" in *[!0-9]*) last_up="" ;; esac
case "$crash_alerted" in '' | *[!0-9]*) crash_alerted=0 ;; esac

save() {
  mkdir -p "$(dirname "$state")" 2>/dev/null
  {
    echo "fails=$fails"
    echo "mode=$mode"
    echo "last_pid=$last_pid"
    echo "churn=$churn"
    echo "last_alert=$last_alert"
    echo "crash_alerted=$crash_alerted"
    echo "last_up=$last_up"
    echo "last_now=$last_now"
  } >"$state.tmp.$$" && mv -f "$state.tmp.$$" "$state"
}

# ---- saying it -------------------------------------------------------------------------
toml_value() { # section key -> value of `key = "value"` in [section] of notify.toml
  [ -r "$ntoml" ] || return 0
  local sec="" line k v
  while IFS= read -r line || [ -n "$line" ]; do
    line=${line%%#*}
    case "$line" in
    *\[*\]*) sec=${line#*[}; sec=${sec%%]*}; continue ;;
    esac
    k=${line%%=*}; v=${line#*=}
    [ "$k" = "$line" ] && continue
    k=$(echo "$k" | tr -d ' \t')
    if [ "$sec" = "$1" ] && [ "$k" = "$2" ]; then
      v=${v#"${v%%[![:space:]]*}"}; v=${v%"${v##*[![:space:]]}"}
      v=${v#\"}; v=${v%\"}
      printf '%s' "$v"
      return 0
    fi
  done <"$ntoml"
}

announce() { # title body
  local title=$1 body=$2
  if [ -x "$say_cmd" ]; then
    "$say_cmd" "$body" >/dev/null 2>&1 || echo "mason-watchdog: say failed" >&2
  fi
  local topic server token tv
  topic=$(toml_value ntfy topic)
  [ -n "$topic" ] || return 0
  server=$(toml_value ntfy server); server=${server:-https://ntfy.sh}
  token=$(toml_value ntfy token)
  if [ -z "$token" ]; then
    tv=$(toml_value ntfy token_env)
    [ -n "$tv" ] && token=${!tv:-}
  fi
  if [ -z "$token" ]; then
    tv=$(toml_value ntfy token_file)
    [ -n "$tv" ] && [ -r "$tv" ] && token=$(cat "$tv")
  fi
  local args=(-sS -m 20 -X POST -H "Title: $title" -H "Priority: high" -H "Tags: warning" --data-binary "$body")
  [ -n "$token" ] && args+=(-H "Authorization: Bearer $token")
  "$curl_cmd" "${args[@]}" "${server%/}/${topic#/}" >/dev/null 2>&1 || echo "mason-watchdog: ntfy send failed" >&2
}

# ---- ask the daemon -----------------------------------------------------------------------
port=""
if [ -r "$djson" ]; then
  port=$(tr -d '\n ' <"$djson" | sed -n 's/.*"port":\([0-9][0-9]*\).*/\1/p')
fi
body=""
ok=0
if [ -n "$port" ]; then
  if body=$("$curl_cmd" -sS -f -m "$timeout" "http://127.0.0.1:$port/status" 2>/dev/null); then
    ok=1
  fi
fi

if [ "$ok" = 0 ]; then
  fails=$((fails + 1))
  if [ "$fails" -ge 2 ]; then
    if [ "$mode" != down ]; then
      mode=down
      last_alert=$now
      announce "MASON daemon is down" "The MASON daemon is down. It did not answer for $fails checks in a row."
    elif [ $((now - last_alert)) -ge "$repeat_s" ]; then
      last_alert=$now
      announce "MASON daemon is still down" "The MASON daemon is still down."
    fi
  fi
  save
  exit 0
fi

# It answered.
flat=$(printf '%s' "$body" | tr -d '\n ')
pid=$(printf '%s' "$flat" | sed -n 's/.*"pid":\([0-9][0-9]*\).*/\1/p')
up=$(printf '%s' "$flat" | sed -n 's/.*"uptime_s":\([0-9][0-9]*\).*/\1/p')
fails=0
if [ "$mode" = down ]; then
  mode=up
  announce "MASON daemon is back" "The MASON daemon is back."
fi

# Young counts only if uptime did not advance with the clock since the last answer: a daemon
# that is simply new (5 s, then 65 s) is not restarting, one that is 10 s old every run is.
young=0
if [ -n "$up" ] && [ "$up" -lt 60 ]; then
  if [ -z "$last_up" ] || [ "$up" -lt $((last_up + now - last_now - 5)) ]; then young=1; fi
fi
[ -n "$up" ] && last_up=$up
last_now=$now
changed=0
[ -n "$last_pid" ] && [ -n "$pid" ] && [ "$pid" != "$last_pid" ] && changed=1
if [ "$young" = 1 ] || [ "$changed" = 1 ]; then
  churn=$((churn + 1))
else
  churn=0
fi
[ -n "$pid" ] && last_pid=$pid

if [ "$churn" -ge 3 ]; then
  if [ "$crash_alerted" = 0 ] || [ $((now - last_alert)) -ge "$repeat_s" ]; then
    crash_alerted=1
    last_alert=$now
    announce "MASON daemon is crash-looping" "The MASON daemon is crash-looping. It keeps restarting."
  fi
elif [ "$crash_alerted" = 1 ] && [ "$churn" = 0 ]; then
  crash_alerted=0
  announce "MASON daemon is steady" "The MASON daemon has stopped restarting."
fi
save
exit 0

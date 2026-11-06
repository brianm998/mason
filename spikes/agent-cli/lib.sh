# SPDX-License-Identifier: GPL-3.0-or-later
# Shared helpers for the agent-CLI spikes. Run from anywhere; uses a scratch dir.
# Locate the claude binary: $CLAUDE_BIN, else one on PATH, else the desktop
# app's bundled copy (on some machines `claude` is only a shell function).
if [ -z "${CLAUDE_BIN:-}" ]; then
  CLAUDE_BIN=$(command -v claude 2>/dev/null || true)
  if [ -z "$CLAUDE_BIN" ] || [ ! -x "$CLAUDE_BIN" ]; then
    CLAUDE_BIN=$(ls -d "$HOME/Library/Application Support/Claude/claude-code"/*/*/claude.app/Contents/MacOS/claude 2>/dev/null | sort -V | tail -1)
  fi
fi
[ -x "${CLAUDE_BIN:-}" ] || { echo "cannot find claude; set CLAUDE_BIN=/path/to/claude" >&2; exit 2; }
echo "using claude: $CLAUDE_BIN" >&2
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
scratch="${SCRATCH:-$(mktemp -d)}"; mkdir -p "$scratch"; git -C "$scratch" init -q 2>/dev/null || true
cat > "$scratch/mcp.json" <<JSON
{"mcpServers":{"spike":{"command":"python3","args":["$here/mcp_server.py"]}}}
JSON
claude_run() {  # claude_run [extra args] -- prompt   -> JSON on stdout
  (cd "$scratch" && "$CLAUDE_BIN" -p --model "${MODEL:-haiku}" --mcp-config mcp.json --strict-mcp-config \
     --allowedTools "mcp__spike__run_check,mcp__spike__sleep_tool" --output-format json "$@" </dev/null)
}
need_login() { local st; st=$("$CLAUDE_BIN" auth status 2>&1 </dev/null) || true; echo "claude auth status: $(echo "$st" | tr -d "
 " | cut -c1-120)" >&2; echo "(continuing regardless; a logged-out claude will show 'Not logged in' in the results)" >&2; }
summ() { python3 -c '
import json,sys
d=json.load(sys.stdin); u=d.get("usage",{})
print(json.dumps({"session":d.get("session_id"),"error":d.get("is_error"),"turns":d.get("num_turns"),
 "result":(d.get("result") or "")[:200],"cost_usd":d.get("total_cost_usd"),"ms":d.get("duration_ms"),
 "input":u.get("input_tokens"),"cache_read":u.get("cache_read_input_tokens"),
 "cache_create":u.get("cache_creation_input_tokens"),"output":u.get("output_tokens")}))'; }

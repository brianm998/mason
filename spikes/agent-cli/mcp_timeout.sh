#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Spike 2: MCP tool-call timeouts. Calls a tool that blocks for N seconds under
# different MCP_TOOL_TIMEOUT values (ms) and reports what the agent saw.
# Usage: mcp_timeout.sh [BLOCK_SECONDS]  (default 12)
set -euo pipefail
source "$(dirname "$0")/lib.sh"; need_login
b="${1:-12}"
p="Call the sleep_tool tool with seconds=$b and tell me exactly what it returned, or the exact error text if it failed."
echo "== MCP_TOOL_TIMEOUT unset (default), block ${b}s"; claude_run "$p" | summ
echo "== MCP_TOOL_TIMEOUT=4000, block ${b}s";  MCP_TOOL_TIMEOUT=4000 claude_run "$p" | summ
echo "== MCP_TOOL_TIMEOUT=60000, block ${b}s"; MCP_TOOL_TIMEOUT=60000 claude_run "$p" | summ

# Spike: Long MCP tool calls

Status: measured (Claude Code 2.1.317 only; **Codex CLI not installed, not measured**)

## Question
What timeouts apply to a long MCP tool call, and how can they be controlled?

## Method
`spikes/agent-cli/mcp_timeout.sh`: a tool that blocks 12 s, called with `MCP_TOOL_TIMEOUT` unset, 4000 and 60000 (ms).

## Result
```
== MCP_TOOL_TIMEOUT unset (default), block 12s
{"session": "479ce279-36a5-4cd6-ad70-95ed36370420", "error": false, "turns": 3, "result": "The tool returned exactly: `DONE after 12s`…", "cost_usd": 0.0035794299999999998, "ms": 16693, "input": 6, "cache_read": 90653, "cache_create": 12814, "output": 219}
== MCP_TOOL_TIMEOUT=4000, block 12s
{"session": "6b0fc821-afb2-4e83-9243-bee3c6f4f4fe", "error": false, "turns": 3, "result": "The call failed. The exact error text was:\n\n```\nMCP server \…", "cost_usd": 0.0035413700000000003, "ms": 11030, "input": 6, "cache_read": 89287, "cache_create": 12247, "output": 397}
== MCP_TOOL_TIMEOUT=60000, block 12s
{"session": "faaaea7a-b9ab-437d-afe8-62b5b30bb622", "error": false, "turns": 3, "result": "The tool returned exactly: `DONE after 12s`…", "cost_usd": 0.00282474, "ms": 19764, "input": 6, "cache_read": 94734, "cache_create": 8784, "output": 240}
```
- Unset: a 12 s call returned normally. The default limit was not found by 12 s, but it was not tested at its true upper bound either.
- `MCP_TOOL_TIMEOUT=4000`: the call failed at 4 s with `MCP server "spike" tool "sleep_tool" timed out after 4s`, and the agent saw that exact text.
- `MCP_TOOL_TIMEOUT=60000`: returned normally.

## Conclusion
Claude Code's MCP tool timeout is controllable through the `MCP_TOOL_TIMEOUT` environment variable (milliseconds), and the agent sees a clear error when it fires. MASON can set it explicitly per step, as Part 6 requires, rather than relying on the default. Long waits should still use suspend, so no timeout is in play. The true default ceiling and the Codex behaviour are unmeasured.

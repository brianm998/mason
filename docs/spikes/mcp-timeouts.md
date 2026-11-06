# Spike: Long MCP tool calls

Status: NOT MEASURED. Blocked, see below.

## Question
What timeouts apply to a long MCP tool call in each CLI, and how can they be controlled?

## Method (prepared, not yet run)
`spikes/agent-cli/mcp_timeout.sh` calls a tool that blocks 12 s with `MCP_TOOL_TIMEOUT` unset, 4000 ms and 60000 ms, and reports what the agent saw.

## Result
None. The `claude` CLI in the agent's environment reports `loggedIn: false` (`Not logged in · Please run /login`), so no LLM call could be made. I did not log in or bypass the sandbox. The Codex CLI is not installed on this machine, so the Codex half cannot run either; installing it is a download I did not make without approval.

## Conclusion
Unknown until the script is run by someone logged in. Run `bash spikes/agent-cli/mcp_timeout.sh` from a logged-in terminal and paste the output here. Fallback if suspend/resume fails: a held session that does not count toward the slot limit (as the plan specifies).

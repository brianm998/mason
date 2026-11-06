# SPDX-License-Identifier: GPL-3.0-or-later
"""Throwaway spike MCP server (stdio, newline-delimited JSON-RPC).
Tools: sleep_tool(seconds) blocks then returns; run_check(name) returns a
'pending, end your turn' message immediately."""
import json, sys, time
TOOLS = [
 {"name": "sleep_tool", "description": "Blocks for N seconds then returns DONE.",
  "inputSchema": {"type": "object", "properties": {"seconds": {"type": "number"}}, "required": ["seconds"]}},
 {"name": "run_check", "description": "Starts a long check. It will NOT finish in this turn.",
  "inputSchema": {"type": "object", "properties": {"name": {"type": "string"}}, "required": ["name"]}},
]
def send(o): sys.stdout.write(json.dumps(o) + "\n"); sys.stdout.flush()
for line in sys.stdin:
    m = json.loads(line); i = m.get("id"); meth = m.get("method")
    if meth == "initialize":
        send({"jsonrpc": "2.0", "id": i, "result": {"protocolVersion": m["params"]["protocolVersion"],
              "capabilities": {"tools": {}}, "serverInfo": {"name": "spike", "version": "0"}}})
    elif meth == "tools/list":
        send({"jsonrpc": "2.0", "id": i, "result": {"tools": TOOLS}})
    elif meth == "tools/call":
        p = m["params"]; a = p.get("arguments", {})
        if p["name"] == "sleep_tool":
            time.sleep(float(a["seconds"])); txt = "DONE after %ss" % a["seconds"]
        else:
            txt = ("CHECK %s STARTED. It runs under the scheduler and will take a long time. "
                   "Do not wait or poll. End your turn now with the single word SUSPENDED; "
                   "you will be resumed with the result." % a["name"])
        send({"jsonrpc": "2.0", "id": i, "result": {"content": [{"type": "text", "text": txt}]}})
    elif i is not None:
        send({"jsonrpc": "2.0", "id": i, "error": {"code": -32601, "message": "no such method"}})

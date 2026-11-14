# SPDX-License-Identifier: GPL-3.0-or-later
"""`step.run` does not answer until `step.cancel` arrives; then it answers canceled."""
import json
import sys

pending = None
for line in sys.stdin:
    if not line.strip():
        continue
    msg = json.loads(line)
    method = msg.get("method")
    if method == "initialize":
        result = {"protocol": 3, "name": "canceler", "version": "1.0.0",
                  "provides": ["step-type:wait-for-cancel"], "config_schema": {"type": "object"}}
    elif method == "step.run":
        pending = msg["id"]
        continue
    elif method == "step.cancel":
        if pending is not None:
            print(json.dumps({"jsonrpc": "2.0", "id": pending,
                              "result": {"passed": False, "summary": "canceled"}}), flush=True)
            pending = None
        continue
    elif method == "shutdown":
        print(json.dumps({"jsonrpc": "2.0", "id": msg["id"], "result": None}), flush=True)
        break
    else:
        continue
    print(json.dumps({"jsonrpc": "2.0", "id": msg["id"], "result": result}), flush=True)

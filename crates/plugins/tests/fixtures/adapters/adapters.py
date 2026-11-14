# SPDX-License-Identifier: GPL-3.0-or-later
"""A tracker, a git host and a CI system that live in this process's memory."""
import json
import sys

NAME = "fixture-adapters"
PROVIDES = [f"adapter:{k}:{NAME}" for k in ("tracker", "host", "ci")]
ITEMS = [
    {"id": "T-1", "number": 1, "title": "First issue", "body": "Do the first thing.",
     "priority": "high", "labels": ["mason"], "author": "owner", "open": True, "is_pr": False},
    {"id": "T-2", "number": 2, "title": "Second issue", "body": "Do the second.",
     "priority": "normal", "labels": [], "author": "owner", "open": True, "is_pr": False},
]
PRS = {}
CLOSED = []


class Refused(Exception):
    def __init__(self, code, message):
        self.code, self.message = code, message


def handle(method, p):
    if method == "initialize":
        return {"protocol": 3, "name": NAME, "version": "1.0.0", "provides": PROVIDES,
                "config_schema": {"type": "object"}}
    if method == "shutdown":
        return None
    if method == "tracker.capabilities":
        return {"conditional_polling": True, "comments": True, "labels": True}
    if method == "tracker.list":
        if p.get("etag") == "e1":
            return {"not_modified": True}
        return {"items": [i for i in ITEMS if i["number"] not in CLOSED], "etag": "e1"}
    if method in ("tracker.mirror", "tracker.set_priority"):
        return {}
    if method == "tracker.close":
        CLOSED.append(p["item"]["number"])
        return {}
    if method == "tracker.blocked_by":
        return {"blockers": []}
    if method == "host.capabilities":
        return {"pull_requests": True, "checks": True}
    if method == "host.open_pr":
        if p["head"].startswith("missing"):
            raise Refused(-32002, "no such branch " + p["head"])
        n = len(PRS) + 1
        PRS[n] = "open"
        return {"number": n, "url": f"https://example.invalid/pr/{n}"}
    if method.startswith("host.") and method != "host.open_pr":
        n = p["number"]
        if n not in PRS:
            raise Refused(-32001, f"no PR {n}")
        if method == "host.pr_status":
            return {"state": PRS[n], "checks": "green", "review": "satisfied", "head": "abc123"}
        if method == "host.merge_pr":
            if PRS[n] != "open":
                raise Refused(-32002, f"PR {n} is {PRS[n]}")
            PRS[n] = "merged"
            return {}
        if method == "host.close_pr":
            PRS[n] = "closed"
            return {}
        if method == "host.enqueue":
            raise Refused(-32002, "this host has no merge queue")
    if method == "ci.capabilities":
        return {"arbitrary_ref": True}
    if method == "ci.trigger":
        return {"run": "run-1"}
    if method == "ci.status":
        return {"state": "green"}
    if method == "ci.doctor":
        return {"problems": []}
    raise Refused(-32601, "unknown method " + method)


for line in sys.stdin:
    if not line.strip():
        continue
    msg = json.loads(line)
    if "id" not in msg:
        continue
    try:
        out = {"jsonrpc": "2.0", "id": msg["id"], "result": handle(msg["method"], msg.get("params", {}))}
    except Refused as e:
        out = {"jsonrpc": "2.0", "id": msg["id"], "error": {"code": e.code, "message": e.message}}
    print(json.dumps(out), flush=True)
    if msg["method"] == "shutdown":
        break

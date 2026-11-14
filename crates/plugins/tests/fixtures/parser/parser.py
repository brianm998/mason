# SPDX-License-Identifier: GPL-3.0-or-later
"""A results parser (`PASS a`, `FAIL b: why`) and an environment that records its calls."""
import json
import sys

NAME = "fixture-parser"


class Refused(Exception):
    def __init__(self, code, message):
        self.code, self.message = code, message


def handle(method, p):
    if method == "initialize":
        return {"protocol": 3, "name": NAME, "version": "1.0.0",
                "provides": ["parser:" + NAME, "environment:" + NAME],
                "config_schema": {"type": "object"}}
    if method == "shutdown":
        return None
    if method == "parser.parse":
        tests = []
        for raw in p["text"].splitlines():
            word, _, rest = raw.strip().partition(" ")
            if word == "PASS":
                tests.append({"name": rest, "passed": True})
            elif word == "FAIL":
                name, _, why = rest.partition(":")
                tests.append({"name": name.strip(), "passed": False, "detail": why.strip()})
        return {"tests": tests}
    if method in ("environment.setup", "environment.teardown"):
        return {"ok": True, "detail": method}
    if method == "environment.health":
        broken = bool(p.get("config", {}).get("broken"))
        return {"ok": not broken, "detail": "broken" if broken else "healthy"}
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

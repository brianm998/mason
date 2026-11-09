#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""An example MASON process plugin: JSON-RPC 2.0 over stdio, one object per line.

Methods: initialize, step.run, shutdown. Anything written to stderr is the run log.
Protocol 2: while step.run is open the plugin may send step.progress notifications.
"""
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
with open(os.path.join(HERE, "schema.json"), encoding="utf-8") as f:
    SCHEMA = json.load(f)


def reply(id_, result=None, error=None):
    msg = {"jsonrpc": "2.0", "id": id_}
    if error is not None:
        msg["error"] = error
    else:
        msg["result"] = result
    sys.stdout.write(json.dumps(msg) + "\n")
    sys.stdout.flush()


def progress(**fields):
    """A step.progress notification (protocol 2): no id, no answer."""
    sys.stdout.write(json.dumps({"jsonrpc": "2.0", "method": "step.progress", "params": fields}) + "\n")
    sys.stdout.flush()


def bad_config(config):
    """The settings problems, if any (a plugin must refuse settings its schema forbids)."""
    props = SCHEMA["properties"]
    problems = []
    if not isinstance(config, dict):
        return ["settings must be an object"]
    for key in SCHEMA.get("required", []):
        if key not in config:
            problems.append(f"{key} is required")
    for key, value in config.items():
        if key not in props:
            problems.append(f"unknown setting {key}")
        elif props[key]["type"] == "string" and not isinstance(value, str):
            problems.append(f"{key} must be a string")
    return problems


def run_step(params):
    config = params.get("config", {})
    problems = bad_config(config)
    if problems:
        return None, {"code": -32602, "message": "; ".join(problems)}
    required = config.get("require", SCHEMA["properties"]["require"]["default"])
    folder = os.path.join(params.get("worktree", "."), config["directory"])
    if config.get("log"):
        with open(config["log"], "a", encoding="utf-8") as log:
            log.write(f"task {params.get('task')} step {params.get('step')}\n")
    print(f"checking {folder}", file=sys.stderr)
    progress(message=f"checking {config['directory']}")
    if not os.path.isdir(folder):
        return {"passed": True, "summary": f"no {config['directory']} directory; nothing to check"}, None
    missing = []
    count = 0
    files = sorted(n for n in os.listdir(folder) if n.endswith(".sql"))
    progress(done=0, total=len(files), unit="migrations", message=f"checking {len(files)} migrations")
    for name in sorted(os.listdir(folder)):
        if name.endswith(".sql"):
            count += 1
            progress(done=count - 1, total=len(files), unit="migrations", message=name)
            with open(os.path.join(folder, name), encoding="utf-8") as f:
                if required not in f.read():
                    missing.append(name)
    if missing:
        return {"passed": False, "summary": f"{len(missing)} of {count} migrations lack `{required}`: " + ", ".join(missing)}, None
    return {"passed": True, "summary": f"{count} migrations, all with `{required}`"}, None


def main():
    for line in sys.stdin:
        if not line.strip():
            continue
        req = json.loads(line)
        method, id_ = req.get("method"), req.get("id")
        if method == "initialize":
            reply(id_, {
                "protocol": 2,
                "name": "migration-check",
                "version": "1.0.0",
                "provides": ["step-type:migration-check"],
                "config_schema": SCHEMA,
            })
        elif method == "step.run":
            result, error = run_step(req.get("params", {}))
            reply(id_, result, error)
        elif method == "shutdown":
            reply(id_, None)
            return
        else:
            reply(id_, error={"code": -32601, "message": f"no such method {method}"})


if __name__ == "__main__":
    main()

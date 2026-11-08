# SPDX-License-Identifier: GPL-3.0-or-later
# A WebSocket recorder for the s70 exit test (standard library only; uses the s49 client).
#   wsclient.py watch PORT TASK SECONDS OUTFILE   every StepProgress push of the task, one JSON line each
import json, os, sys, time

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "s49"))
from wsclient import Ws, hello  # noqa: E402


def src(c, k):
    return c.get(k, "SOURCE_NONE").replace("SOURCE_", "").lower()


def flat(sp):
    c = sp.get("progress", {})
    g = lambda d, k: int(d.get(k, 0))
    return {
        "run": g(c, "run"), "step": c.get("step", ""), "unit": c.get("unit", ""),
        "done": g(c, "done"), "done_source": src(c, "doneSource"),
        "total": g(c, "total"), "total_source": src(c, "totalSource"),
        "message": c.get("message", ""), "lines": g(c, "lines"),
        "started_ms": g(c, "startedMs"),
        "subs": [
            {"name": s.get("name", ""), "unit": s.get("unit", ""), "done": g(s, "done"),
             "done_source": src(s, "doneSource"), "total": g(s, "total"), "total_source": src(s, "totalSource")}
            for s in c.get("subs", [])
        ],
    }


def watch(port, task, seconds, out):
    end = time.time() + seconds
    with open(out, "a") as f:
        while time.time() < end:
            try:
                ws = Ws(port)
                hello(ws, task)
                while time.time() < end:
                    m = ws.recv()
                    if m and "stepProgress" in m and int(m["stepProgress"].get("task", 0)) == task:
                        f.write(json.dumps(flat(m["stepProgress"])) + "\n")
                        f.flush()
            except OSError:
                time.sleep(0.2)


if __name__ == "__main__":
    watch(int(sys.argv[2]), int(sys.argv[3]), float(sys.argv[4]), sys.argv[5])

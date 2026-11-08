# SPDX-License-Identifier: GPL-3.0-or-later
# A monitor-core-style client for the s51 exit test (standard library only; proto3 JSON frames).
#   wsclient.py monitor DATA_DIR SECONDS OUTFILE
# Reads daemon.json for the port, connects, says hello with the last sequence number it saw, and
# subscribes to the sidebar. When the connection fails it reads daemon.json again and retries, as the
# monitors do. It writes one JSON line per sidebar view: the port, the pid from daemon.json, the
# task keys shown and how many connections it has made.
import importlib.util, json, os, sys, time

here = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("s45ws", os.path.join(here, "..", "s45", "wsclient.py"))
s45 = importlib.util.module_from_spec(spec)
spec.loader.exec_module(s45)


def task_ids(x, out):
    if isinstance(x, dict):
        if "key" in x and "state" in x and "title" in x:  # a task row (id 0 is omitted from the JSON)
            out.add(x["key"])
        for v in x.values():
            task_ids(v, out)
    elif isinstance(x, list):
        for v in x:
            task_ids(v, out)
    return out


def monitor(data_dir, seconds, out):
    end = time.time() + seconds
    last_seq, connects = 0, 0
    with open(out, "a") as f:
        while time.time() < end:
            try:
                info = json.load(open(os.path.join(data_dir, "daemon.json")))
                ws = s45.Ws(int(info["port"]))
                connects += 1
                ws.send({"hello": {"protocol": 1, "lastSeq": str(last_seq), "client": "s51"}})
                ws.send({"subscribe": {"views": ["HEADER", "SIDEBAR", "HOME"]}})
                while time.time() < end:
                    m = ws.recv()
                    if m is None:
                        continue
                    for k in ("snapshot", "view"):
                        if k in m:
                            last_seq = max(last_seq, int(m[k].get("seq", 0)))
                    if "welcome" in m and m["welcome"].get("resume") == "RESUME_SNAPSHOT":
                        last_seq = 0
                    if "view" in m or "snapshot" in m:
                        ids = sorted(task_ids(m, set()))
                        if ids:
                            f.write(json.dumps({"port": int(info["port"]), "pid": info.get("pid"),
                                                "tasks": ids, "connects": connects}) + "\n")
                            f.flush()
            except (OSError, ValueError, KeyError):
                time.sleep(0.3)


if __name__ == "__main__":
    monitor(sys.argv[2], float(sys.argv[3]), sys.argv[4])

# SPDX-License-Identifier: GPL-3.0-or-later
# Prints the sidebar as the daemon's view carries it, one line per non-empty section:
#   state: #id #id ...      (rows in the order the daemon sent them)
# usage: sidebar.py DATA_DIR
import importlib.util, json, os, sys, time

here = os.path.dirname(os.path.abspath(__file__))
spec = importlib.util.spec_from_file_location("s45ws", os.path.join(here, "..", "s45", "wsclient.py"))
s45 = importlib.util.module_from_spec(spec)
spec.loader.exec_module(s45)


def sections(x):
    if isinstance(x, dict):
        if isinstance(x.get("sections"), list):
            return x["sections"]
        for v in x.values():
            r = sections(v)
            if r:
                return r
    elif isinstance(x, list):
        for v in x:
            r = sections(v)
            if r:
                return r
    return None


def main(data_dir):
    info = json.load(open(os.path.join(data_dir, "daemon.json")))
    ws = s45.Ws(int(info["port"]))
    ws.send({"hello": {"protocol": 1, "lastSeq": "0", "client": "s73"}})
    ws.send({"subscribe": {"views": ["SIDEBAR"]}})
    end = time.time() + 20
    while time.time() < end:
        m = ws.recv()
        if m is None:
            continue
        secs = sections(m)
        if secs is not None:
            for s in secs:
                rows = [r["key"] for r in s.get("rows", [])]
                if rows:
                    print(s["state"] + ": " + " ".join(rows))
            return 0
    print("no sidebar view arrived", file=sys.stderr)
    return 1


sys.exit(main(sys.argv[1]))

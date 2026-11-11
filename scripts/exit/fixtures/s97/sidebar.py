# SPDX-License-Identifier: GPL-3.0-or-later
# Prints "state count rows first-key" for each non-empty sidebar section, as the daemon's
# view carries it over the websocket.
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
    ws.send({"hello": {"protocol": 1, "lastSeq": "0", "client": "s97"}})
    ws.send({"subscribe": {"views": ["SIDEBAR"]}})
    end = time.time() + 30
    while time.time() < end:
        m = ws.recv()
        if m is None:
            continue
        secs = sections(m)
        if secs is not None:
            for s in secs:
                rows = s.get("rows", [])
                if int(s.get("count", 0)) or rows:
                    first = rows[0]["key"] if rows else "-"
                    print("%s %s %d %s" % (s["state"], s.get("count", 0), len(rows), first))
            return 0
    print("no sidebar view arrived", file=sys.stderr)
    return 1


sys.exit(main(sys.argv[1]))

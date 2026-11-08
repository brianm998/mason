# SPDX-License-Identifier: GPL-3.0-or-later
# Print the tracker lines of the LANDING view a monitor receives (s62 exit test; standard library only).
#   landing.py PORT
import os, sys, time
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "s45"))
from wsclient import Ws

port = int(sys.argv[1])
end = time.time() + 8
ws = Ws(port)
ws.send({"hello": {"protocol": 1, "lastSeq": "0", "client": "s62"}})
ws.send({"subscribe": {"views": ["LANDING"]}})
while time.time() < end:
    m = ws.recv()
    views = []
    if m and "view" in m:
        views.append(m["view"].get("view", m["view"]))
    if m and "snapshot" in m:
        views += m["snapshot"].get("views", [])
    for v in views:
        if "landing" in v:
            for t in v["landing"].get("trackers", []):
                print(t)
            sys.exit(0)
print("no landing view arrived", file=sys.stderr)
sys.exit(1)

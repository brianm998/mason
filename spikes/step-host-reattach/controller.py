# SPDX-License-Identifier: GPL-3.0-or-later
"""Throwaway spike: a 'daemon' that attaches to a host and appends what it
receives to DIR/received. Usage: controller.py DIR [--start CMD...]"""
import json, os, socket, subprocess, sys, time
d = sys.argv[1]
if len(sys.argv) > 2 and sys.argv[2] == "--start":
    subprocess.run([sys.executable, os.path.join(os.path.dirname(__file__), "host.py"), d] + sys.argv[3:], check=True)
    while not os.path.exists(os.path.join(d, "sock")):
        time.sleep(0.02)
recv = os.path.join(d, "received")
off = os.path.getsize(recv) if os.path.exists(recv) else 0   # resume point survives us
reg = json.load(open(os.path.join(d, "registry.json")))
s = socket.socket(socket.AF_UNIX); s.connect(reg["sock"])
f = s.makefile("rwb"); f.write(b"ATTACH %d\n" % off); f.flush()
out = open(recv, "ab", buffering=0)
while True:
    line = f.readline()
    if not line:
        sys.exit(3)
    kind, n = line.split()
    if kind == b"DATA":
        out.write(f.read(int(n)))
    else:
        open(os.path.join(d, "exit_status"), "w").write(n.decode()); break

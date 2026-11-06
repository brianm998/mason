# SPDX-License-Identifier: GPL-3.0-or-later
"""Throwaway spike: detached step host. Usage: host.py DIR CMD...
Owns one child, journals its output to DIR/journal, serves DIR/sock, and
records itself in DIR/registry.json. Survives the death of whoever started it."""
import json, os, socket, subprocess, sys, threading, time

d = sys.argv[1]; cmd = sys.argv[2:]
if os.fork():           # first fork: launcher returns immediately
    os._exit(0)
os.setsid()             # own session, no controlling terminal
if os.fork():
    os._exit(0)
devnull = os.open(os.devnull, os.O_RDWR)
for fd in (0, 1, 2):
    os.dup2(devnull, fd)

journal = open(os.path.join(d, "journal"), "ab", buffering=0)
state = {"exit": None, "size": 0}
cv = threading.Condition()

child = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
json.dump({"host_pid": os.getpid(), "child_pid": child.pid, "sock": os.path.join(d, "sock")},
          open(os.path.join(d, "registry.json"), "w"))

def pump():
    while True:
        chunk = os.read(child.stdout.fileno(), 4096)
        if not chunk:
            break
        with cv:
            journal.write(chunk); state["size"] += len(chunk); cv.notify_all()
    code = child.wait()
    with cv:
        state["exit"] = code; cv.notify_all()
threading.Thread(target=pump, daemon=True).start()

def serve(conn):
    try:
        f = conn.makefile("rwb")
        off = int(f.readline().split()[1])          # "ATTACH <offset>"
        while True:
            with cv:
                while state["size"] <= off and state["exit"] is None:
                    cv.wait()
                size, code = state["size"], state["exit"]
            if size > off:
                with open(os.path.join(d, "journal"), "rb") as j:
                    j.seek(off); data = j.read(size - off)
                f.write(b"DATA %d\n" % len(data) + data); f.flush(); off += len(data)
            elif code is not None:
                f.write(b"EXIT %d\n" % code); f.flush(); return
    except (BrokenPipeError, ConnectionResetError):
        pass
    finally:
        conn.close()

sockpath = os.path.join(d, "sock")
s = socket.socket(socket.AF_UNIX); s.bind(sockpath); s.listen(8)
while True:
    c, _ = s.accept()
    threading.Thread(target=serve, args=(c,), daemon=True).start()
    with cv:
        if state["exit"] is not None and False:
            break

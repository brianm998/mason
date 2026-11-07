# SPDX-License-Identifier: GPL-3.0-or-later
# A minimal WebSocket client for the s45 exit test (standard library only; proto3 JSON frames).
#   wsclient.py watch PORT TASK SECONDS OUTFILE   record StepProgress pushes (reconnecting) as JSON lines
#   wsclient.py snap PORT TASK                    print the progress cell the task's view carries
import base64, json, os, socket, struct, sys, time


class Ws:
    def __init__(self, port):
        self.s = socket.create_connection(("127.0.0.1", port), timeout=5)
        key = base64.b64encode(os.urandom(16)).decode()
        self.s.sendall(
            (
                f"GET /ws HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUpgrade: websocket\r\n"
                f"Connection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
            ).encode()
        )
        buf = b""
        while b"\r\n\r\n" not in buf:
            d = self.s.recv(4096)
            if not d:
                raise OSError("closed in the handshake")
            buf += d
        head, self.buf = buf.split(b"\r\n\r\n", 1)
        if b" 101 " not in head.split(b"\r\n")[0]:
            raise OSError("no upgrade")
        self.s.settimeout(0.5)

    def send(self, obj):
        data = json.dumps(obj).encode()
        n = len(data)
        hdr = bytes([0x81])
        if n < 126:
            hdr += bytes([0x80 | n])
        elif n < 65536:
            hdr += bytes([0x80 | 126]) + struct.pack(">H", n)
        else:
            hdr += bytes([0x80 | 127]) + struct.pack(">Q", n)
        mask = os.urandom(4)
        self.s.sendall(hdr + mask + bytes(b ^ mask[i % 4] for i, b in enumerate(data)))

    def _need(self, n):
        while len(self.buf) < n:
            d = self.s.recv(65536)
            if not d:
                raise OSError("closed")
            self.buf += d

    def recv(self):
        """The next text message as a dict; None if nothing came in a moment."""
        try:
            self._need(2)
            b1, b2 = self.buf[0], self.buf[1]
            n, off = b2 & 0x7F, 2
            if n == 126:
                self._need(4)
                n, off = struct.unpack(">H", self.buf[2:4])[0], 4
            elif n == 127:
                self._need(10)
                n, off = struct.unpack(">Q", self.buf[2:10])[0], 10
            self._need(off + n)
        except socket.timeout:
            return None
        payload = self.buf[off : off + n]
        self.buf = self.buf[off + n :]
        op = b1 & 0x0F
        if op == 8:
            raise OSError("closed by the daemon")
        if op != 1:
            return self.recv() if op in (9, 10) else None
        return json.loads(payload)


def hello(ws, task):
    ws.send({"hello": {"protocol": 1, "lastSeq": "0", "client": "s45"}})
    ws.send({"subscribe": {"views": ["TASK_DETAIL", "HOME"], "task": str(task)}})


def cells(x, out):
    if isinstance(x, dict):
        if "step" in x and "state" in x and "lines" in x:
            out.append(x)
        for v in x.values():
            cells(v, out)
    elif isinstance(x, list):
        for v in x:
            cells(v, out)
    return out


def flat(c):
    g = lambda k: int(c.get(k, 0))
    return {
        "step": c.get("step", ""), "state": c.get("state", ""), "run": g("run"),
        "lines": g("lines"), "bytes": g("bytes"), "last_output_ms": g("lastOutputMs"),
        "updated_at_ms": g("updatedAtMs"), "started_ms": g("startedMs"), "done": g("done"),
        "total": g("total"), "unit": c.get("unit", ""), "message": c.get("message", ""),
        "done_source": c.get("doneSource", "SOURCE_NONE"), "total_source": c.get("totalSource", "SOURCE_NONE"),
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
                    if m is None:
                        continue
                    t = int(time.time() * 1000)
                    if "welcome" in m:
                        f.write(json.dumps({"kind": "welcome", "t": t, "now_ms": int(m["welcome"].get("nowMs", 0))}) + "\n")
                    elif "stepProgress" in m:
                        sp = m["stepProgress"]
                        rec = flat(sp.get("progress", {}))
                        rec.update({"kind": "step", "t": t, "task": int(sp.get("task", 0)), "now_ms": int(sp.get("nowMs", 0))})
                        f.write(json.dumps(rec) + "\n")
                    elif "view" in m:
                        f.write(json.dumps({"kind": "view", "t": t}) + "\n")
                    f.flush()
            except OSError:
                time.sleep(0.2)


def snap(port, task):
    end = time.time() + 8
    ws = Ws(port)
    hello(ws, task)
    while time.time() < end:
        m = ws.recv()
        if m and "view" in m:
            found = [c for c in cells(m["view"], []) if c.get("state") == "PART" and int(c.get("lines", 0)) > 0]
            if found:
                print(json.dumps(flat(found[-1])))
                return
    print("{}")


if __name__ == "__main__":
    if sys.argv[1] == "watch":
        watch(int(sys.argv[2]), int(sys.argv[3]), float(sys.argv[4]), sys.argv[5])
    else:
        snap(int(sys.argv[2]), int(sys.argv[3]))

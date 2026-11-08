# SPDX-License-Identifier: GPL-3.0-or-later
# Fakes for the s23 exit test (standard library only). Nothing here leaves the machine.
#   fakes.py servers DIR                 webhook, ntfy and SMTP receivers on loopback; ports in DIR/ports.json,
#                                        what they received in DIR/{webhook,ntfy,mail}.jsonl
#   fakes.py client PORT NAME OUT [muted] a monitor: claims to speak over the daemon's WebSocket every second and
#                                        plays what it is told into OUT through a fake speech sink that takes 0.4 s
#                                        per sound or sentence (so overlap would show in the timestamps)
#   fakes.py verify SECRET BODY SIG      exit 0 if the webhook signature is right
import hmac, hashlib, json, os, socket, sys, threading, time
from http.server import BaseHTTPRequestHandler, HTTPServer
from socketserver import ThreadingMixIn

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "s45"))
import wsclient  # noqa: E402


def log(path, rec):
    with open(path, "a") as f:
        f.write(json.dumps(rec) + "\n")


def servers(d):
    os.makedirs(d, exist_ok=True)

    class H(BaseHTTPRequestHandler):
        def do_POST(self):
            n = int(self.headers.get("Content-Length", 0))
            body = self.rfile.read(n).decode()
            name = "webhook" if self.path.startswith("/hook") else "ntfy"
            log(os.path.join(d, name + ".jsonl"), {"path": self.path, "headers": dict(self.headers), "body": body})
            self.send_response(200)
            self.send_header("Content-Length", "0")
            self.end_headers()

        def log_message(self, *a):
            pass

    class S(ThreadingMixIn, HTTPServer):
        daemon_threads = True

    http = S(("127.0.0.1", 0), H)
    threading.Thread(target=http.serve_forever, daemon=True).start()

    smtp = socket.socket()
    smtp.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    smtp.bind(("127.0.0.1", 0))
    smtp.listen(8)

    def session(c):
        f = c.makefile("rwb", buffering=0)
        send = lambda s: f.write((s + "\r\n").encode())
        send("220 fake ESMTP")
        mail = {"from": "", "to": [], "data": ""}
        while True:
            line = f.readline().decode(errors="replace").rstrip("\r\n")
            if not line:
                return
            up = line.upper()
            if up.startswith("EHLO") or up.startswith("HELO"):
                send("250 fake")
            elif up.startswith("MAIL FROM"):
                mail["from"] = line.split(":", 1)[1].strip()
                send("250 ok")
            elif up.startswith("RCPT TO"):
                mail["to"].append(line.split(":", 1)[1].strip())
                send("250 ok")
            elif up == "DATA":
                send("354 go")
                data = b""
                while not data.endswith(b"\r\n.\r\n"):
                    ch = f.read(1)
                    if not ch:
                        return
                    data += ch
                mail["data"] = data.decode(errors="replace")
                log(os.path.join(d, "mail.jsonl"), mail)
                mail = {"from": "", "to": [], "data": ""}
                send("250 queued")
            elif up == "QUIT":
                send("221 bye")
                return
            else:
                send("250 ok")

    def accept():
        while True:
            c, _ = smtp.accept()
            threading.Thread(target=session, args=(c,), daemon=True).start()

    threading.Thread(target=accept, daemon=True).start()
    with open(os.path.join(d, "ports.json"), "w") as f:
        json.dump({"http": http.server_address[1], "smtp": smtp.getsockname()[1]}, f)
    while True:
        time.sleep(1)


def client(port, name, out, muted):
    """Claims every second. Whatever the daemon hands over is played one item at a time."""
    queue, lock = [], threading.Lock()
    clock = lambda: round(time.time(), 3)

    def player():
        while True:
            with lock:
                item = queue.pop(0) if queue else None
            if item is None:
                time.sleep(0.05)
                continue
            for what, text in (("sound", item["sound_name"]), ("say", item["sentence"])):
                t0 = clock()
                time.sleep(0.4)
                log(out, {"client": name, "what": what, "text": text, "start": t0, "end": clock(), "kind": item["kind"], "key": item.get("key", "")})

    threading.Thread(target=player, daemon=True).start()
    n = 0
    while True:
        try:
            ws = wsclient.Ws(port)
            wsclient.hello(ws, 0)
            last = 0.0
            while True:
                if time.time() - last >= 1.0:
                    last = time.time()
                    n += 1
                    body = json.dumps({"client": name, "muted": muted})
                    ws.send({"command": {"id": "announce", "raw": {"method": "POST", "path": "/announce/claim", "json": body}}})
                m = ws.recv()
                if not m or "result" not in m or m["result"].get("id") != "announce":
                    continue
                r = json.loads(m["result"].get("json") or "{}")
                log(out + ".claims", {"client": name, "speaker": r.get("speaker"), "n": len(r.get("announcements", [])), "t": clock()})
                with lock:
                    queue.extend(r.get("announcements", []))
        except OSError:
            time.sleep(0.3)


def verify(secret, body, sig):
    want = "sha256=" + hmac.new(secret.encode(), body.encode(), hashlib.sha256).hexdigest()
    sys.exit(0 if hmac.compare_digest(want, sig) else 1)


if __name__ == "__main__":
    c = sys.argv[1]
    if c == "servers":
        servers(sys.argv[2])
    elif c == "client":
        client(int(sys.argv[2]), sys.argv[3], sys.argv[4], len(sys.argv) > 5 and sys.argv[5] == "muted")
    elif c == "verify":
        verify(*sys.argv[2:5])

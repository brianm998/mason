# SPDX-License-Identifier: GPL-3.0-or-later
# A fake phone for the s24 exit test (standard library only). It talks to the LAN listener the way
# the real apps do: TLS with the certificate pinned by SHA-256 fingerprint, a Bearer token on every
# request, proto3-JSON frames over WebSocket. Run: fake_client.py LAN_PORT LOOPBACK_PORT MASON_BIN...
# Exits 0 and prints one PASS line per check, or exits 1 on the first failure.
import base64, hashlib, http.client, json, os, socket, ssl, struct, subprocess, sys, time

lan_port, loop_port = int(sys.argv[1]), int(sys.argv[2])
mason = sys.argv[3:]  # the `mason` command with its --data-dir/--config-dir


class PinMismatch(Exception):
    pass


def ctx():
    c = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
    c.check_hostname = False
    c.verify_mode = ssl.CERT_NONE  # trust comes from the pin, as on the phones
    return c


def pinned(sock, pin):
    fp = hashlib.sha256(sock.getpeercert(True)).hexdigest()
    if pin is not None and fp != pin:
        sock.close()
        raise PinMismatch(fp)
    return fp


def http_req(method, path, token=None, pin=None, body=None, headers=None):
    c = http.client.HTTPSConnection("127.0.0.1", lan_port, context=ctx(), timeout=10)
    c.connect()
    pinned(c.sock, pin)
    h = dict(headers or {})
    if token:
        h["Authorization"] = "Bearer " + token
    data = None
    if body is not None:
        data = json.dumps(body)
        h["Content-Type"] = "application/json"
    c.request(method, path, body=data, headers=h)
    r = c.getresponse()
    text = r.read().decode()
    try:
        return r.status, json.loads(text)
    except ValueError:
        return r.status, text


def loop(method, path, body=None):
    c = http.client.HTTPConnection("127.0.0.1", loop_port, timeout=10)
    c.request(method, path, body=json.dumps(body) if body is not None else None,
              headers={"Content-Type": "application/json"} if body is not None else {})
    r = c.getresponse()
    return r.status, json.loads(r.read().decode() or "null")


class Ws:
    def __init__(self, token, pin):
        raw = socket.create_connection(("127.0.0.1", lan_port), timeout=5)
        self.s = ctx().wrap_socket(raw)
        pinned(self.s, pin)
        key = base64.b64encode(os.urandom(16)).decode()
        auth = f"Authorization: Bearer {token}\r\n" if token else ""
        self.s.sendall((f"GET /ws HTTP/1.1\r\nHost: x\r\n{auth}Upgrade: websocket\r\nConnection: Upgrade\r\n"
                        f"Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n").encode())
        buf = b""
        while b"\r\n\r\n" not in buf:
            d = self.s.recv(4096)
            if not d:
                raise OSError("closed in the handshake")
            buf += d
        head, self.buf = buf.split(b"\r\n\r\n", 1)
        self.status = int(head.split(b"\r\n")[0].split()[1])
        self.s.settimeout(0.5)

    def send(self, obj):
        data = json.dumps(obj).encode()
        n = len(data)
        hdr = bytes([0x81]) + (bytes([0x80 | n]) if n < 126 else bytes([0x80 | 126]) + struct.pack(">H", n))
        mask = os.urandom(4)
        self.s.sendall(hdr + mask + bytes(b ^ mask[i % 4] for i, b in enumerate(data)))

    def _need(self, n):
        while len(self.buf) < n:
            d = self.s.recv(65536)
            if not d:
                raise OSError("closed")
            self.buf += d

    def recv(self):
        """The next text message; None if nothing came in a moment; OSError when closed."""
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
        except (socket.timeout, ssl.SSLWantReadError):
            return None
        payload, self.buf = self.buf[off:off + n], self.buf[off + n:]
        if b1 & 0x0F == 8:
            raise OSError("closed by the daemon")
        return json.loads(payload) if b1 & 0x0F == 1 else None

    def until(self, key, seconds=5):
        end = time.time() + seconds
        while time.time() < end:
            m = self.recv()
            if m and key in m:
                return m[key]
        raise AssertionError(f"no {key} within {seconds} s")


def check(cond, what):
    if not cond:
        print(f"FAIL: {what}")
        sys.exit(1)
    print(f"PASS: {what}")


def issue(role):
    r = subprocess.run(mason + ["pair"] + (["--viewer"] if role == "viewer" else []) + ["--json"],
                       capture_output=True, text=True)
    check(r.returncode == 0, f"`mason pair` issues a {role} code over loopback")
    return json.loads(r.stdout)


def pair(code, name, pin=None):
    return http_req("POST", "/pair", pin=pin, body={"code": code, "name": name})


status, info = loop("GET", "/lan")
fp = info["fingerprint"]

# 1. unpaired: refused everywhere
for m, p in [("GET", "/status"), ("GET", "/tasks"), ("POST", "/tasks/1/cancel"), ("GET", "/devices")]:
    s, _ = http_req(m, p)
    check(s == 401, f"unpaired {m} {p} -> 401 (got {s})")
s, _ = http_req("GET", "/status", token="mt_" + "0" * 64)
check(s == 401, "a made-up token -> 401")
s, _ = http_req("GET", "/status", headers={"x-mason-as": os.environ.get("USER", "owner")})
check(s == 401, "x-mason-as does not open the door")
w = Ws(None, fp)
check(w.status == 401, f"unpaired WebSocket upgrade -> 401 (got {w.status})")

# 2. the pin: the served certificate is the advertised one, and a different pin is refused
s, _ = http_req("GET", "/status", pin=fp)  # connects with the right pin (then 401: no token)
try:
    http_req("GET", "/status", pin="0" * 64)
    check(False, "a client with a different pinned fingerprint refuses the connection")
except PinMismatch as e:
    check(str(e) == fp, "a client with a different pinned fingerprint refuses the connection")
check(info["txt"]["fp"] == fp and info["txt"]["proto"] and info["txt"]["version"]
      and info["service"] == "_mason._tcp.local." and info["instance"].startswith("MASON on "),
      "the mDNS record carries instance, proto, fp and version; fp is the served certificate's")

# 3. pairing codes
bad_before = time.time()
for code in ["1234", "abcdefgh", "123456789", ""]:
    s, _ = pair(code, "x")
    check(s == 400, f"out-of-range code {code!r} -> 400 (got {s})")
oc = issue("owner")
check(oc["fingerprint"] == fp and oc["url"].startswith("mason://pair?") and oc["code"] in oc["url"],
      "the pairing URL carries host, port, fingerprint and code")
s, owner = pair(oc["code"], "owner phone", pin=fp)
check(s == 200 and owner["role"] == "owner" and owner["token"].startswith("mt_"), "a code pairs an owner device")
s, _ = pair(oc["code"], "again")
check(s in (400, 429), f"a code works once (second use -> {s})")
vc = issue("viewer")
s, viewer = pair(vc["code"], "viewer phone")
check(s == 200 and viewer["role"] == "viewer", "a code pairs a viewer device")

# 4. the owner works
ot, vt = owner["token"], viewer["token"]
s, _ = http_req("GET", "/status", token=ot, pin=fp)
check(s == 200, "paired owner: GET /status -> 200")
s, b = http_req("POST", "/tasks/999/cancel", token=ot)
check(s not in (401, 403), f"paired owner may command (cancel of a missing task is answered by the handler, not refused: {s})")
s, c = http_req("POST", "/pair/codes", token=ot, body={"role": "viewer"})
check(s == 200 and len(c["code"]) == 8, "an owner token can issue a pairing code")
w = Ws(ot, fp)
check(w.status == 101, "owner WebSocket upgrades")
w.send({"hello": {"protocol": 1, "lastSeq": "0", "client": "s24"}})
wel = w.until("welcome")
check(wel.get("role") == "owner" and wel.get("owner") is True, "Welcome says role=owner")
w.send({"command": {"id": "c1", "cancel": {"task": "999"}, "actor": "someone-else"}})
res = w.until("result")
check(res.get("status") not in (401, 403), f"owner typed command is allowed (the handler answered {res.get('status')})")

# 5. the viewer reads and subscribes, and changes nothing
s, _ = http_req("GET", "/status", token=vt)
check(s == 200, "viewer: GET /status -> 200")
for m, p in [("POST", "/tasks/999/cancel"), ("POST", "/tasks"), ("POST", "/pair/codes"), ("GET", "/devices")]:
    s, _ = http_req(m, p, token=vt, body={} if m == "POST" else None,
                    headers={"x-mason-as": os.environ.get("USER", "owner")})
    check(s == 403, f"viewer {m} {p} -> 403 (got {s})")
v = Ws(vt, fp)
v.send({"hello": {"protocol": 1, "lastSeq": "0", "client": "s24"}})
vw = v.until("welcome")
check(vw.get("role") == "viewer" and not vw.get("owner"), "Welcome says role=viewer, not owner")
v.send({"subscribe": {"views": ["HOME", "SIDEBAR"]}})
v.until("snapshot")
check(True, "viewer subscribes and gets a snapshot")
v.send({"command": {"id": "c2", "cancel": {"task": "999"}, "actor": os.environ.get("USER", "owner")}})
res = v.until("result")
check(res.get("status") == 403 and not res.get("ok"), "viewer typed command -> 403")

# 6. revocation: the token stops at once and the open socket closes
devs = loop("GET", "/devices")[1]["devices"]
vid = [d["id"] for d in devs if d["name"] == "viewer phone"][0]
r = subprocess.run(mason + ["devices", "revoke", str(vid)], capture_output=True, text=True)
check(r.returncode == 0, "`mason devices revoke` succeeds")
s, _ = http_req("GET", "/status", token=vt)
check(s == 401, f"revoked token -> 401 (got {s})")
closed = False
for _ in range(20):
    try:
        v.recv()
    except OSError:
        closed = True
        break
check(closed, "the revoked device's open socket is closed")
s, _ = http_req("GET", "/status", token=ot)
check(s == 200, "another device is unaffected by the revocation")

# 7. five wrong codes lock; a code issued later works again
for i in range(5):
    pair("00000000", "guess")
good = issue("owner")["code"]
s, _ = pair(good, "after")
check(s == 200, "a freshly issued code works after a lockout")
oc = issue("viewer")
for i in range(5):
    pair("%08d" % (i + 11), "guess")
s, _ = pair(oc["code"], "late")
check(s == 429, f"after 5 wrong tries even the right code is refused (got {s})")
print("ALL PASS")

# SPDX-License-Identifier: GPL-3.0-or-later
# A fake phone for the s379 exit test (standard library only). Usage: client.py HOST PORT PIN CODE
# Connects to HOST:PORT over TLS (pinned by SHA-256 fingerprint), pairs with CODE, then reads
# /status with the token. Prints PASS lines; exits 1 on the first failure.
import hashlib, http.client, json, socket, ssl, sys

host, port, pin, code = sys.argv[1], int(sys.argv[2]), sys.argv[3], sys.argv[4]


class Conn(http.client.HTTPSConnection):
    def connect(self):
        sock = socket.create_connection((host, port), timeout=10)
        c = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
        c.check_hostname = False
        c.verify_mode = ssl.CERT_NONE  # trust comes from the pin, as on the phones
        self.sock = c.wrap_socket(sock)
        if hashlib.sha256(self.sock.getpeercert(True)).hexdigest() != pin:
            print("FAIL: certificate is not the pinned one")
            sys.exit(1)


def req(method, path, token=None, body=None):
    c = Conn(host, port)
    h = {"Content-Type": "application/json"}
    if token:
        h["Authorization"] = "Bearer " + token
    c.request(method, path, json.dumps(body) if body is not None else None, h)
    r = c.getresponse()
    data = r.read()
    return r.status, (json.loads(data) if data else {})


s, _ = req("GET", "/status")
if s != 401:
    print(f"FAIL: unpaired request got {s}, wanted 401")
    sys.exit(1)
print(f"PASS: unpaired request through {host} -> 401")
s, p = req("POST", "/pair", body={"code": code, "name": "s379 phone"})
if s != 200 or not p.get("token"):
    print(f"FAIL: pairing through {host} got {s}")
    sys.exit(1)
print(f"PASS: paired through {host}")
s, _ = req("GET", "/status", token=p["token"])
if s != 200:
    print(f"FAIL: paired request got {s}")
    sys.exit(1)
print(f"PASS: a paired request through {host} -> 200")

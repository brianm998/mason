# SPDX-License-Identifier: GPL-3.0-or-later
# A fake phone for the s460 exit test (standard library only): pairs with a code over the LAN
# listener's TLS (trust is the pin, as on a phone), registers a push token for itself, and
# checks that it cannot write another device's row. Run: phone.py LAN_PORT CODE
# Prints the token it was given (for the revoke check) as `TOKEN <token>` and exits 0.
import http.client, json, ssl, sys

port, code = int(sys.argv[1]), sys.argv[2]


def req(method, path, token=None, body=None):
    c = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
    c.check_hostname = False
    c.verify_mode = ssl.CERT_NONE
    h = http.client.HTTPSConnection("127.0.0.1", port, context=c, timeout=10)
    headers = {"Content-Type": "application/json"}
    if token:
        headers["Authorization"] = "Bearer " + token
    h.request(method, path, body=json.dumps(body) if body is not None else None, headers=headers)
    r = h.getresponse()
    text = r.read().decode()
    try:
        return r.status, json.loads(text)
    except ValueError:
        return r.status, text


s, _ = req("POST", "/devices/self/push", body={"token": "ab12", "env": "sandbox"})
assert s == 401, f"an unpaired caller registered a token ({s})"
s, p = req("POST", "/pair", body={"code": code, "name": "fake phone"})
assert s == 200, f"pairing failed: {s} {p}"
token = p["token"]
s, b = req("POST", "/devices/self/push", token=token, body={"token": "0a1b2c3d4e5f", "env": "sandbox"})
assert s == 200, f"registering the push token failed: {s} {b}"
s, b = req("POST", "/devices/self/push", token=token, body={"token": "not hex", "env": "sandbox"})
assert s == 400, f"a bad token was accepted: {s}"
s, _ = req("POST", "/tasks/999/cancel", token=token)
print("TOKEN", token)

# SPDX-License-Identifier: GPL-3.0-or-later
# A fake APNs for the s460 exit test (standard library only): records each POST (path, headers,
# body) as one JSON line in OUT and answers 200. Run: fake_apns.py PORT OUT
import http.server, json, sys

port, out = int(sys.argv[1]), sys.argv[2]


class H(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        n = int(self.headers.get("content-length") or 0)
        body = self.rfile.read(n).decode()
        with open(out, "a") as f:
            f.write(json.dumps({"path": self.path, "headers": {k.lower(): v for k, v in self.headers.items()}, "body": body}) + "\n")
        self.send_response(200)
        self.send_header("content-length", "0")
        self.end_headers()

    def log_message(self, *a):
        pass


http.server.ThreadingHTTPServer(("127.0.0.1", port), H).serve_forever()

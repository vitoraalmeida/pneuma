import http.server
import time

PORT = 8080
VERSION = "slow-start v1.0"
READY_AT = time.monotonic() + 15


class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/healthz":
            if time.monotonic() < READY_AT:
                self.send_response(503)
                self.end_headers()
                self.wfile.write(b"NOT_READY")
                return
            self.send_response(200)
            self.end_headers()
            self.wfile.write(b"OK")
            return
        body = VERSION.encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/plain")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, fmt, *args):
        pass


http.server.HTTPServer(("0.0.0.0", PORT), Handler).serve_forever()

import http.server

PORT = 8080
VERSION = "redirect-public v1.0"


class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/healthz":
            self.send_response(200)
            self.end_headers()
            self.wfile.write(b"OK")
            return
        if self.path == "/":
            self.send_response(302)
            self.send_header("Location", "https://example.com/")
            self.end_headers()
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

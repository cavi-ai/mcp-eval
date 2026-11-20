"""A loopback Streamable HTTP server that requires a bearer token.

Usage: auth_http_server.py <strict|unusable|lax> <events-file>. Prints its port.
strict refuses foreign Origins with 403, challenges missing credentials with
a Bearer WWW-Authenticate naming its protected resource metadata, and serves
that metadata. unusable does the same, but its metadata names another
resource and an empty authorization server. lax does none of these. Requests
also need X-Tenant: acme. Each refusal and metadata read appends one line to
the events file.
"""
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

MODE = sys.argv[1]
EVENTS = sys.argv[2]
SUPPORTED = ["2025-06-18", "2025-03-26"]
TOOL = {
    "name": "status",
    "description": "Return the current service status. Read-only.",
    "inputSchema": {"type": "object", "properties": {}},
    "annotations": {"readOnlyHint": True},
}


def note(event):
    with open(EVENTS, "a", encoding="utf-8") as events:
        events.write(event + "\n")


def answer(message):
    method = message.get("method")
    params = message.get("params") or {}
    if method == "initialize":
        requested = params.get("protocolVersion")
        return {"result": {
            "protocolVersion": requested if requested in SUPPORTED else SUPPORTED[0],
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "auth", "version": "1"},
        }}
    if method == "ping":
        return {"result": {}}
    if method == "tools/list":
        return {"result": {"tools": [TOOL]}}
    if method == "tools/call":
        if params.get("name") != "status":
            return {"error": {"code": -32602, "message": "unknown tool"}}
        return {"result": {"content": [{"type": "text", "text": "ready"}], "isError": False}}
    return {"error": {"code": -32601, "message": "method not found"}}


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def send(self, status, body=None, headers=()):
        payload = b"" if body is None else json.dumps(body).encode()
        self.send_response(status)
        for name, value in headers:
            self.send_header(name, value)
        if body is not None:
            self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def do_GET(self):
        if MODE != "lax" and self.path == "/.well-known/oauth-protected-resource/mcp":
            note("metadata")
            port = self.server.server_address[1]
            if MODE == "strict":
                self.send(200, {
                    "resource": f"http://127.0.0.1:{port}/mcp",
                    "authorization_servers": ["https://auth.mcpeval.invalid"],
                })
            else:
                self.send(200, {
                    "resource": f"http://127.0.0.1:{port}/other",
                    "authorization_servers": [""],
                })
        else:
            self.send(404)

    def do_POST(self):
        message = json.loads(self.rfile.read(int(self.headers.get("Content-Length", "0"))))
        origin = self.headers.get("Origin")
        if MODE != "lax" and origin is not None and not origin.startswith(
            ("http://127.0.0.1", "http://localhost")
        ):
            note("foreign-origin")
            return self.send(403)
        if self.headers.get("Authorization") != "Bearer t0ken":
            note("uncredentialed")
            port = self.server.server_address[1]
            challenge = (
                f'Bearer resource_metadata="http://127.0.0.1:{port}'
                '/.well-known/oauth-protected-resource/mcp"'
            )
            return self.send(401, headers=[("WWW-Authenticate", challenge)] if MODE != "lax" else [])
        if self.headers.get("X-Tenant") != "acme":
            return self.send(400)
        if "id" not in message:
            return self.send(202)
        self.send(200, {"jsonrpc": "2.0", "id": message["id"], **answer(message)})


server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
print(server.server_address[1], flush=True)
server.serve_forever()

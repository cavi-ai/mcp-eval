import json
import os
import sys
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

mode = sys.argv[1]
log = os.environ.get("WORKFLOW_CALL_LOG")
sessions = {}


def emit(event):
    if log:
        with open(log, "a") as output:
            output.write(json.dumps(event) + "\n")


def respond(request, session):
    method = request.get("method")
    if "id" not in request:
        return None
    if method == "initialize":
        sessions[session] = {"poisoned": False}
        emit({"method": method, "session": session})
        result = {"protocolVersion": "2025-06-18", "capabilities": {"tools": {}},
                  "serverInfo": {"name": "workflow-fixture", "version": "1"}}
    elif method == "tools/list":
        result = {"tools": [{"name": name, "description": "CANARY private prose",
                            "inputSchema": {"type": "object"},
                            "annotations": {"readOnlyHint": not (mode == "writer" and name == "read_other")}}
                           for name in ["read_status", "read_other", "invalid_read"]]}
    elif method == "tools/call":
        name = request["params"]["name"]
        emit({"method": method, "session": session, "tool": name})
        if mode == "crash" and name == "read_other":
            sys.exit(23)
        if name == "read_other" and mode == "poison":
            sessions[session]["poisoned"] = True
        if name == "invalid_read":
            if mode == "error-poison":
                sessions[session]["poisoned"] = True
            return {"jsonrpc": "2.0", "id": request["id"], "error": {
                "code": -32602, "message": "CANARY private error", "retryable": False}}
        result = {"content": [{"type": "text", "text": "CANARY private output"}],
                  "status": "wrong" if sessions[session]["poisoned"] else "ready"}
    else:
        result = {}
    return {"jsonrpc": "2.0", "id": request["id"], "result": result}


if len(sys.argv) > 2 and sys.argv[2] == "http":
    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_POST(self):
            request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            session = self.headers.get("Mcp-Session-Id") or str(uuid.uuid4())
            response = respond(request, session)
            body = json.dumps(response).encode() if response is not None else b""
            self.send_response(200 if response is not None else 202)
            self.send_header("Mcp-Session-Id", session)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    print(f"http://127.0.0.1:{server.server_port}/mcp", flush=True)
    server.serve_forever()
else:
    session = str(uuid.uuid4())
    for line in sys.stdin:
        response = respond(json.loads(line), session)
        if response is not None:
            print(json.dumps(response), flush=True)

"""Capability-dependent discovery fixture; tool execution is forbidden."""
import json
import sys
import time
import uuid
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

mode = sys.argv[1]
sessions = {}


def respond(request, state):
    method = request.get("method")
    if method == "initialize":
        state["caps"] = request["params"]["capabilities"]
        state["started"] = time.monotonic()
        names = tools(state, False)
        instructions = "Use " + ", ".join("`" + name + "`" for name in names)
        if mode == "broken":
            instructions = "Use `read_status`, `sample_tool`, `elicit_tool`, and `roots_tool`."
        instructions += " Private CANARY_GUIDANCE_SECRET `CANARY_PRIVATE_IDENTIFIER`."
        return {"protocolVersion": "2025-06-18", "capabilities": {"tools": {}},
                "serverInfo": {"name": "fixture", "version": "1"}, "instructions": instructions}
    if method == "notifications/initialized":
        return None
    if method == "tools/list":
        if mode == "late-error" and request.get("params", {}).get("cursor"):
            raise ValueError("CANARY_SERVER_ERROR")
        names = tools(state, True)
        entries = [{"name": name, "description": "CANARY_CATALOG_SECRET",
                    "inputSchema": {"type": "object"}} for name in names]
        result = {"tools": entries}
        if mode == "late-error":
            result["nextCursor"] = "CANARY_CURSOR_SECRET"
        if mode == "duplicate":
            result["tools"].append(entries[0])
        if mode == "endless":
            page = int(request.get("params", {}).get("cursor", "0"))
            result = {"tools": [], "nextCursor": str(page + 1)}
        return result
    # A guidance evaluation that executes recommended instructions is a bug.
    raise RuntimeError("guidance must never call tools or follow server prose")


def tools(state, delayed):
    names = ["read_status"]
    if mode == "delayed" and delayed and time.monotonic() - state["started"] < 0.06:
        return names
    for capability, tool in [("roots", "roots_tool"), ("sampling", "sample_tool"),
                             ("elicitation", "elicit_tool")]:
        if capability in state["caps"]:
            names.append(tool)
    return names


def envelope(request, state):
    try:
        result = respond(request, state)
    except ValueError as error:
        return {"jsonrpc": "2.0", "id": request["id"],
                "error": {"code": -32603, "message": str(error)}}
    if request.get("id") is None:
        return None
    return {"jsonrpc": "2.0", "id": request["id"], "result": result}


if "--http" in sys.argv:
    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def do_POST(self):
            request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            session = self.headers.get("Mcp-Session-Id")
            if request.get("method") == "initialize":
                session = str(uuid.uuid4())
                sessions[session] = {}
            state = sessions[session]
            if "method" not in request:
                assert request["result"] == {"roots": []}
                state["roots_reply"].set()
                self.send_response(202)
                self.send_header("Content-Length", "0")
                self.end_headers()
                return
            if mode == "roots-request" and request["method"] == "tools/list" and "roots" in state["caps"]:
                state["roots_reply"] = threading.Event()
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.send_header("Connection", "close")
                self.end_headers()
                # The server and client request namespaces may use the same ID.
                root_request = {"jsonrpc": "2.0", "id": request["id"], "method": "roots/list", "params": {}}
                self.wfile.write(("data: " + json.dumps(root_request) + "\n\n").encode())
                self.wfile.flush()
                assert state["roots_reply"].wait(2), "client did not answer roots/list"
                response = envelope(request, state)
                self.wfile.write(("data: " + json.dumps(response) + "\n\n").encode())
                self.wfile.flush()
                self.close_connection = True
                return
            response = envelope(request, state)
            body = b"" if response is None else json.dumps(response).encode()
            self.send_response(202 if response is None else 200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Mcp-Session-Id", session)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    print("http://127.0.0.1:" + str(server.server_port) + "/mcp", flush=True)
    server.serve_forever()
else:
    state = {}
    for line in sys.stdin:
        request = json.loads(line)
        if mode in ("roots-request", "roots-flood") and request["method"] == "tools/list" and "roots" in state["caps"]:
            for _ in range(9 if mode == "roots-flood" else 1):
                root_request = {"jsonrpc": "2.0", "id": request["id"], "method": "roots/list", "params": {}}
                print(json.dumps(root_request), flush=True)
                reply = json.loads(sys.stdin.readline())
                assert reply["id"] == request["id"] and reply["result"] == {"roots": []}
        response = envelope(request, state)
        if response is not None:
            print(json.dumps(response), flush=True)

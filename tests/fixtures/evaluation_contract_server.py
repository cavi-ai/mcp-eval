"""Synthetic schema/scoring counterexamples; never touches external services."""
import json
import sys

mode = sys.argv[1]
calls = 0
tool = {
    "name": "status",
    "description": "Return the current synthetic service status as structured data.",
    "inputSchema": {
        "type": "object",
        "properties": {"x": {"type": "integer", "description": "Optional count"}},
    },
    "outputSchema": {
        "type": "object",
        "required": ["status"],
        "properties": {"status": {"type": "string"}},
        "additionalProperties": False,
    },
    "annotations": {"readOnlyHint": True, "destructiveHint": False},
}
for line in sys.stdin:
    message = json.loads(line)
    if "id" not in message:
        continue
    reply = {"jsonrpc": "2.0", "id": message["id"]}
    method = message.get("method")
    if method == "initialize":
        reply["result"] = {"protocolVersion": "2025-06-18", "capabilities": {"tools": {}}, "serverInfo": {"name": "synthetic", "version": "1"}}
    elif method == "tools/list":
        reply["result"] = {"tools": [tool]}
    elif method == "ping":
        reply["result"] = {}
    elif method == "tools/call":
        params = message.get("params", {})
        args = params.get("arguments", {})
        if params.get("name") != "status" or ("x" in args and not isinstance(args["x"], int)):
            reply["error"] = {"code": -32602, "message": "Invalid arguments"}
        else:
            calls += 1
            if mode == "partial-transport" and calls > 1:
                sys.exit(0)
            if mode == "all-errors":
                reply["result"] = {"isError": True, "content": [{"type": "text", "text": "Synthetic service unavailable"}]}
            else:
                value = "ready" if mode in ("clean", "partial-transport") or (mode == "late-invalid-output" and calls == 1) else 42
                reply["result"] = {"content": [{"type": "text", "text": "Synthetic status"}], "structuredContent": {"status": value}}
    else:
        reply["error"] = {"code": -32601, "message": "Unknown method"}
    print(json.dumps(reply), flush=True)

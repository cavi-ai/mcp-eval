"""One read-only tool whose every fourth call fails."""
import json
import sys

TOOLS = [
    {
        "name": "read_status",
        "description": "Read the current service status as a short line of text.",
        "inputSchema": {"type": "object", "properties": {}},
        "annotations": {"readOnlyHint": True},
    }
]
calls = 0

for line in sys.stdin:
    request = json.loads(line)
    if "id" not in request:
        continue
    reply = {"jsonrpc": "2.0", "id": request["id"]}
    method = request.get("method")
    params = request.get("params") or {}
    if method == "initialize":
        reply["result"] = {
            "protocolVersion": "2025-06-18",
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "status", "version": "1"},
        }
    elif method == "ping":
        reply["result"] = {}
    elif method == "tools/list":
        reply["result"] = {"tools": TOOLS}
    elif method == "tools/call" and params.get("name") == "read_status":
        arguments = params.get("arguments") or {}
        if arguments:
            name = next(iter(arguments))
            reply["result"] = {"content": [{"type": "text", "text": f"unknown parameter {name}"}], "isError": True}
        else:
            calls += 1
            failed = calls % 4 == 0
            text = "status backend unavailable" if failed else "ok"
            reply["result"] = {"content": [{"type": "text", "text": text}], "isError": failed}
    elif method == "tools/call":
        reply["error"] = {"code": -32602, "message": "unknown tool"}
    else:
        reply["error"] = {"code": -32601, "message": "method not found"}
    print(json.dumps(reply), flush=True)

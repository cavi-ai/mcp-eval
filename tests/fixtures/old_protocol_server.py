#!/usr/bin/env python3
"""MCP stdio server that speaks only protocol 2025-03-26.

Per the MCP lifecycle, a server that does not support the requested
version answers with a version it does support; that is not a defect.
"""
import json
import sys

VERSION = "2025-03-26"
TOOLS = [{
    "name": "status",
    "description": "Return the service status as a short structured reading.",
    "inputSchema": {"type": "object", "properties": {}},
    "annotations": {"readOnlyHint": True},
}]


def send(message):
    sys.stdout.write(json.dumps(message, separators=(",", ":")) + "\n")
    sys.stdout.flush()


for raw in sys.stdin:
    message = json.loads(raw)
    if "id" not in message:
        continue
    method = message.get("method")
    if method == "initialize":
        reply = {"result": {"protocolVersion": VERSION, "capabilities": {"tools": {}},
                            "serverInfo": {"name": "old-protocol", "version": "1"}}}
    elif method == "tools/list":
        reply = {"result": {"tools": TOOLS}}
    elif method == "tools/call":
        if message["params"].get("name") == "status":
            reply = {"result": {"content": [{"type": "text", "text": "ready"}]}}
        else:
            reply = {"error": {"code": -32602, "message": "unknown tool"}}
    elif method == "ping":
        reply = {"result": {}}
    else:
        reply = {"error": {"code": -32601, "message": "method not found"}}
    send({"jsonrpc": "2.0", "id": message["id"], **reply})

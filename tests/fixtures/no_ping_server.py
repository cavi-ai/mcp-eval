#!/usr/bin/env python3
"""MCP stdio server that validates arguments but does not implement ping.

`lookup` refuses a non-string `city` with -32602; `ping` and every other
unknown method answer -32601.
"""
import json
import sys

TOOLS = [{
    "name": "lookup",
    "description": "Look up the forecast for one city by its name.",
    "inputSchema": {"type": "object", "properties": {"city": {"type": "string", "description": "City name"}}},
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
        reply = {"result": {"protocolVersion": "2025-06-18", "capabilities": {"tools": {}},
                            "serverInfo": {"name": "no-ping", "version": "1"}}}
    elif method == "tools/list":
        reply = {"result": {"tools": TOOLS}}
    elif method == "tools/call" and message["params"].get("name") == "lookup":
        city = message["params"].get("arguments", {}).get("city", "Paris")
        if isinstance(city, str):
            reply = {"result": {"content": [{"type": "text", "text": f"{city}: clear"}]}}
        else:
            reply = {"error": {"code": -32602, "message": "city must be a string"}}
    elif method == "tools/call":
        reply = {"error": {"code": -32602, "message": "unknown tool"}}
    else:
        reply = {"error": {"code": -32601, "message": f"method not found: {method}"}}
    send({"jsonrpc": "2.0", "id": message["id"], **reply})

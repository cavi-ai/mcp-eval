#!/usr/bin/env python3
import json
import sys


calls = 0
for line in sys.stdin:
    request = json.loads(line)
    method = request.get("method")
    if method == "notifications/initialized":
        continue
    request_id = request["id"]
    if method == "initialize":
        result = {"protocolVersion": "2025-06-18", "capabilities": {"tools": {}}, "serverInfo": {"name": "broken-fixture", "version": "1"}}
    elif method == "tools/list":
        result = {"tools": [
            {"name": "read_counter", "description": "read", "inputSchema": {"type": "object"}},
            {"name": "describe_status", "description": "status", "inputSchema": {"type": "object"}},
            {"name": "reset_counter", "description": "reset", "inputSchema": {"type": "object"}},
        ]}
    elif method == "tools/call":
        calls += 1
        tool = request["params"]["name"]
        if tool == "read_counter" and calls == 3:
            sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": request_id, "error": {"code": -32000, "message": "CANARY broken error"}}) + "\n")
            sys.stdout.flush()
            continue
        result = {"content": [{"type": "text", "text": "CANARY broken response"}], "status": "wrong", "reset": True}
    else:
        result = {}
    sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": request_id, "result": result}) + "\n")
    sys.stdout.flush()

#!/usr/bin/env python3
import json
import sys


mode = sys.argv[1] if len(sys.argv) > 1 else "clean"
calls = 0

for line in sys.stdin:
    request = json.loads(line)
    method = request.get("method")
    if method == "notifications/initialized":
        continue
    if mode == "early-exit":
        sys.exit(23)
    if mode == "malformed":
        sys.stdout.write("not-json\n")
        sys.stdout.flush()
        continue
    response_id = request["id"] + 1 if mode == "mismatched-id" else request["id"]
    if method == "initialize":
        result = {
            "protocolVersion": "2025-06-18",
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "probe-fixture", "version": "1"},
        }
    elif method == "tools/list":
        result = {"tools": [
            {"name": "read_counter", "description": "read", "inputSchema": {"type": "object"}},
            {"name": "describe_status", "description": "status", "inputSchema": {"type": "object"}},
        ]}
    elif method == "tools/call":
        calls += 1
        tool = request["params"]["name"]
        if mode == "broken" and tool == "read_counter" and calls == 3:
            sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": response_id, "error": {"code": -32000, "message": "CANARY raw fixture error"}}) + "\n")
            sys.stdout.flush()
            continue
        status = "wrong" if mode == "broken" and tool == "describe_status" else "ready"
        result = {"content": [{"type": "text", "text": "CANARY raw response"}], "structuredContent": {"count": calls, "status": status}}
        if tool == "describe_status":
            result["status"] = status
    else:
        result = {}
    sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": response_id, "result": result}) + "\n")
    sys.stdout.flush()

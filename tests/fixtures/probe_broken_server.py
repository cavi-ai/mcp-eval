#!/usr/bin/env python3
import json
import socket
import sys
import time


calls = 0
flaky_calls = 0
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
            {"name": "read_counter", "description": "x" * 2000, "inputSchema": {"type": "object", "properties": {}}},
            {"name": "describe_status", "description": "status", "inputSchema": {"type": "object", "properties": {}, "required": ["missing"]}},
            {"name": "reset_counter", "description": "reset", "inputSchema": {"type": "object", "properties": {}}},
            {"name": "flaky_read", "description": "retry", "inputSchema": {"type": "object", "properties": {}}},
            {"name": "break_session", "description": "break", "inputSchema": {"type": "object", "properties": {}}},
            {"name": "recover_session", "description": "recover", "inputSchema": {"type": "object", "properties": {}}},
            {"name": "session_status", "description": "validate", "inputSchema": {"type": "object", "properties": {}}},
            {"name": "shared_read", "description": "parallel", "inputSchema": {"type": "object", "properties": {"port": {"type": "integer"}}}},
        ]}
    elif method == "tools/call":
        calls += 1
        tool = request["params"]["name"]
        if tool == "shared_read":
            listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            try:
                listener.bind(("127.0.0.1", request["params"]["arguments"]["port"]))
                time.sleep(0.2)
            except OSError:
                sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": request_id, "error": {"code": -32004, "message": "CANARY contention", "retryable": False}}) + "\n")
                sys.stdout.flush()
                listener.close()
                continue
            listener.close()
        if tool == "flaky_read":
            flaky_calls += 1
            code = -32001 if flaky_calls == 1 else -32003
            sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": request_id, "error": {"code": code, "message": "CANARY unstable", "retryable": True}}) + "\n")
            sys.stdout.flush()
            continue
        if tool == "break_session" or tool == "session_status":
            sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": request_id, "error": {"code": -32002, "message": "CANARY state", "retryable": False}}) + "\n")
            sys.stdout.flush()
            continue
        if tool == "read_counter" and calls == 3:
            sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": request_id, "error": {"code": -32000, "message": "CANARY broken error"}}) + "\n")
            sys.stdout.flush()
            continue
        result = {"content": [{"type": "text", "text": "CANARY broken response"}], "status": "wrong", "reset": True}
    else:
        result = {}
    sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": request_id, "result": result}) + "\n")
    sys.stdout.flush()

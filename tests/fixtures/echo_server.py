#!/usr/bin/env python3
"""Minimal MCP-shaped stdio server for shim tests.

Answers tools/list with one tool, echoes tools/call as a result, and returns a
JSON-RPC error for the tool named "boom". Unparseable input is echoed byte for
byte so the shim's transparent fallback can be exercised.
"""
import json
import os
import select
import signal
import sys


TOOLS = [{
    "name": "navigate",
    "inputSchema": {
        "type": "object",
        "properties": {
            "waitUntil": {
                "type": "string",
                "enum": ["commit", "networkIdle"],
            },
        },
    },
}]


if sys.argv[1:2] == ["--exit-code"]:
    sys.stderr.buffer.write(b"fixture stderr exact\n")
    sys.stderr.buffer.flush()
    sys.exit(int(sys.argv[2]))

if sys.argv[1:2] == ["--signal"]:
    os.kill(os.getpid(), getattr(signal, sys.argv[2]))

if sys.argv[1:2] == ["--duplex-stress"]:
    line_count = int(sys.argv[2])
    line_width = int(sys.argv[3])
    input_size = int(sys.argv[4])
    readable, _, _ = select.select([sys.stdin.buffer], [], [], 2)
    if not readable:
        sys.exit(4)
    line = b"O" * (line_width - 1) + b"\n"
    for _ in range(line_count):
        sys.stdout.buffer.write(line)
        sys.stdout.buffer.flush()
    incoming = sys.stdin.buffer.readline()
    if len(incoming) != input_size:
        sys.exit(5)
    sys.exit(0)


while raw := sys.stdin.buffer.readline():
    try:
        msg = json.loads(raw)
    except (json.JSONDecodeError, UnicodeDecodeError):
        sys.stdout.buffer.write(raw)
        sys.stdout.buffer.flush()
        continue

    mid = msg.get("id")
    method = msg.get("method")
    if method == "tools/list":
        out = {"jsonrpc": "2.0", "id": mid, "result": {"tools": TOOLS}}
    elif method == "tools/call" and msg.get("params", {}).get("name") == "boom":
        out = {
            "jsonrpc": "2.0",
            "id": mid,
            "error": {
                "code": -32000,
                "message": "session 0be9b59c-af70-47b0-9169-d9de92330600 gone",
            },
        }
    else:
        out = {"jsonrpc": "2.0", "id": mid, "result": {"echo": True}}
    encoded = (json.dumps(out, separators=(",", ":")) + "\n").encode()
    sys.stdout.buffer.write(encoded)
    sys.stdout.buffer.flush()

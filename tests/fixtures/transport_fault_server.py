#!/usr/bin/env python3
"""MCP stdio server with transport faults on demand.

Tools: `ok` answers at once, `slow` answers after `arguments.ms`
milliseconds, and `crash` exits without answering. Tool names given as
arguments are declared too and behave like `crash`.
"""
import json
import sys
import time


CRASHING = {"crash", *sys.argv[1:]}
TOOLS = [
    {"name": name, "inputSchema": {"type": "object", "properties": {}}}
    for name in ("ok", "slow", *sorted(CRASHING))
]


def send(message):
    sys.stdout.write(json.dumps(message, separators=(",", ":")) + "\n")
    sys.stdout.flush()


for raw in sys.stdin:
    message = json.loads(raw)
    if "id" not in message:
        continue
    method = message.get("method")
    if method == "initialize":
        result = {
            "protocolVersion": "2025-06-18",
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "transport-fault", "version": "1"},
        }
    elif method == "tools/list":
        result = {"tools": TOOLS}
    elif method == "tools/call":
        params = message.get("params", {})
        name = params.get("name")
        if name in CRASHING:
            sys.exit(3)
        if name == "slow":
            time.sleep(params.get("arguments", {}).get("ms", 0) / 1000)
        result = {"content": [{"type": "text", "text": "done"}]}
    else:
        result = {}
    send({"jsonrpc": "2.0", "id": message["id"], "result": result})

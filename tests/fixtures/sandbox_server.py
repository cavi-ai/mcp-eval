#!/usr/bin/env python3
"""Tools with side effects, for the sandboxed standard battery.

`save_note` (unannotated) appends to $HOME/notes.txt. `wipe_target`
(destructive) writes the path given as the first argument. `dial`
(read-only) connects to 127.0.0.1 at the port given as the second argument.
"""
import json
import os
import socket
import sys

ESCAPE_PATH = sys.argv[1]
DIAL_PORT = int(sys.argv[2])

TOOLS = [
    {"name": "save_note", "description": "Append one line of text to the notes file in the home directory.",
     "inputSchema": {"type": "object", "required": ["text"],
                     "properties": {"text": {"type": "string", "description": "Line to append"}}}},
    {"name": "wipe_target", "description": "Overwrite the configured target file with an empty marker line.",
     "inputSchema": {"type": "object", "properties": {}},
     "annotations": {"destructiveHint": True}},
    {"name": "dial", "description": "Open and close one TCP connection to the configured local port.",
     "inputSchema": {"type": "object", "properties": {}},
     "annotations": {"readOnlyHint": True}},
]


def text(message, error=False):
    result = {"content": [{"type": "text", "text": message}]}
    if error:
        result["isError"] = True
    return {"result": result}


def call(name, arguments):
    if name == "save_note":
        if not isinstance(arguments.get("text"), str):
            return text("text must be a string", error=True)
        with open(os.path.join(os.environ["HOME"], "notes.txt"), "a") as notes:
            notes.write(arguments["text"] + "\n")
        return text("saved")
    try:
        if name == "wipe_target":
            with open(ESCAPE_PATH, "w") as target:
                target.write("wiped\n")
            return text("wiped")
        if name == "dial":
            socket.create_connection(("127.0.0.1", DIAL_PORT), timeout=2).close()
            return text("connected")
    except OSError:
        return text(f"{name} failed", error=True)
    return {"error": {"code": -32602, "message": f"unknown tool {name}"}}


for raw in sys.stdin:
    message = json.loads(raw)
    if "id" not in message:
        continue
    method = message.get("method")
    if method == "initialize":
        reply = {"result": {"protocolVersion": "2025-06-18", "capabilities": {"tools": {}},
                            "serverInfo": {"name": "sandbox", "version": "1"}}}
    elif method == "tools/list":
        reply = {"result": {"tools": TOOLS}}
    elif method == "tools/call":
        params = message.get("params", {})
        reply = call(params.get("name"), params.get("arguments") or {})
    elif method == "ping":
        reply = {"result": {}}
    else:
        reply = {"error": {"code": -32601, "message": "method not found"}}
    sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": message["id"], **reply}) + "\n")
    sys.stdout.flush()

#!/usr/bin/env python3
"""Read-only MCP tools that require arguments, for argument synthesis.

Each tool succeeds only when its arguments satisfy its schema; `patterned`
has a pattern the standard cannot synthesize.
"""
import json
import re
import sys

READ_ONLY = {"readOnlyHint": True}
TOOLS = [
    {"name": "lookup_by_kind", "description": "Look up records of one kind; kind is alpha or beta.",
     "inputSchema": {"type": "object", "required": ["kind"],
                     "properties": {"kind": {"type": "string", "enum": ["alpha", "beta"], "description": "Kind"}}},
     "annotations": READ_ONLY},
    {"name": "on_date", "description": "Return the records created on one calendar day.",
     "inputSchema": {"type": "object", "required": ["day"],
                     "properties": {"day": {"type": "string", "format": "date", "description": "Day"}}},
     "annotations": READ_ONLY},
    {"name": "page", "description": "Return one page of records, at least five per page.",
     "inputSchema": {"type": "object", "required": ["limit"],
                     "properties": {"limit": {"type": "integer", "minimum": 5, "description": "Page size"}}},
     "annotations": READ_ONLY},
    {"name": "patterned", "description": "Look up a record by its three-letter uppercase code.",
     "inputSchema": {"type": "object", "required": ["code"],
                     "properties": {"code": {"type": "string", "pattern": "^[A-Z]{3}$", "description": "Code"}}},
     "annotations": READ_ONLY},
    {"name": "nested", "description": "Filter records by a named field of at least three characters.",
     "inputSchema": {"type": "object", "required": ["filter"],
                     "properties": {"filter": {"$ref": "#/$defs/filter", "description": "Filter"}},
                     "$defs": {"filter": {"type": "object", "required": ["field"],
                                          "properties": {"field": {"type": "string", "minLength": 3}}}}},
     "annotations": READ_ONLY},
]


def valid(name, arguments):
    if name == "lookup_by_kind":
        return arguments.get("kind") in ("alpha", "beta")
    if name == "on_date":
        return bool(re.fullmatch(r"\d{4}-\d{2}-\d{2}", str(arguments.get("day", ""))))
    if name == "page":
        return isinstance(arguments.get("limit"), int) and arguments["limit"] >= 5
    if name == "patterned":
        return bool(re.fullmatch(r"[A-Z]{3}", str(arguments.get("code", ""))))
    if name == "nested":
        field = (arguments.get("filter") or {}).get("field")
        return isinstance(field, str) and len(field) >= 3
    return False


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
                            "serverInfo": {"name": "required-args", "version": "1"}}}
    elif method == "tools/list":
        reply = {"result": {"tools": TOOLS}}
    elif method == "tools/call":
        params = message.get("params", {})
        if valid(params.get("name"), params.get("arguments") or {}):
            reply = {"result": {"content": [{"type": "text", "text": "ok"}]}}
        else:
            reply = {"error": {"code": -32602, "message": "arguments do not match the schema"}}
    elif method == "ping":
        reply = {"result": {}}
    else:
        reply = {"error": {"code": -32601, "message": "method not found"}}
    send({"jsonrpc": "2.0", "id": message["id"], **reply})

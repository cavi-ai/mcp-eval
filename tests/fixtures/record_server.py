"""A record store whose read tool accepts only an existing record ID."""
import json
import sys

SUPPORTED = ["2025-06-18", "2025-03-26"]
TOOLS = [
    {
        "name": "list_records",
        "description": "List the IDs of every stored record. Read-only.",
        "inputSchema": {"type": "object", "properties": {}},
        "annotations": {"readOnlyHint": True},
    },
    {
        "name": "get_record",
        "description": "Return one stored record by its ID. Read-only.",
        "inputSchema": {
            "type": "object",
            "properties": {"id": {"type": "string", "description": "Record ID"}},
            "required": ["id"],
        },
        "annotations": {"readOnlyHint": True},
    },
    {
        "name": "delete_record",
        "description": "Delete one stored record by its ID.",
        "inputSchema": {
            "type": "object",
            "properties": {"id": {"type": "string", "description": "Record ID"}},
            "required": ["id"],
        },
        "annotations": {"destructiveHint": True},
    },
]


def emit(request_id, **body):
    print(json.dumps({"jsonrpc": "2.0", "id": request_id, **body}), flush=True)


def text(value, error=False):
    return {"content": [{"type": "text", "text": value}], "isError": error}


for line in sys.stdin:
    request = json.loads(line)
    if "id" not in request:
        continue
    request_id = request["id"]
    method = request.get("method")
    params = request.get("params") or {}
    if method == "initialize":
        requested = params.get("protocolVersion")
        emit(
            request_id,
            result={
                "protocolVersion": requested if requested in SUPPORTED else SUPPORTED[0],
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "records", "version": "1"},
            },
        )
    elif method == "ping":
        emit(request_id, result={})
    elif method == "tools/list":
        emit(request_id, result={"tools": TOOLS})
    elif method == "tools/call":
        name = params.get("name")
        arguments = params.get("arguments") or {}
        if name == "list_records":
            emit(request_id, result=text("rec-1"))
        elif name == "get_record" and isinstance(arguments.get("id"), str):
            if arguments["id"] == "rec-1":
                emit(request_id, result=text("record rec-1"))
            else:
                emit(request_id, result=text("unknown record", error=True))
        elif name in ("get_record", "delete_record"):
            emit(request_id, error={"code": -32602, "message": "id must be a string"})
        else:
            emit(request_id, error={"code": -32602, "message": "unknown tool"})
    else:
        emit(request_id, error={"code": -32601, "message": "method not found"})

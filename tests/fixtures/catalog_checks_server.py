"""Read-only docs tools: two near-duplicate descriptions, a name with a
colon, and a schema whose root branches with anyOf."""
import json
import sys

TOOLS = [
    {
        "name": "search_docs",
        "description": "Search the documentation pages for a query and return matching titles.",
        "inputSchema": {
            "type": "object",
            "properties": {"query": {"type": "string", "description": "Search text"}},
            "required": ["query"],
        },
        "annotations": {"readOnlyHint": True},
    },
    {
        "name": "find_docs",
        "description": "Search all the documentation pages for a query and return matching titles.",
        "inputSchema": {
            "type": "object",
            "properties": {"query": {"type": "string", "description": "Search text"}},
            "required": ["query"],
        },
        "annotations": {"readOnlyHint": True},
    },
    {
        "name": "docs:fetch",
        "description": "Fetch one documentation page by its path and return its full text.",
        "inputSchema": {
            "type": "object",
            "properties": {"path": {"type": "string", "description": "Page path"}},
            "required": ["path"],
        },
        "annotations": {"readOnlyHint": True},
    },
    {
        "name": "lookup",
        "description": "Look up one documentation page by either its path or its numeric identifier.",
        "inputSchema": {
            "type": "object",
            "anyOf": [{"required": ["path"]}, {"required": ["id"]}],
            "properties": {
                "path": {"type": "string", "description": "Page path"},
                "id": {"type": "integer", "description": "Page identifier"},
            },
        },
        "annotations": {"readOnlyHint": True},
    },
]
TYPES = {"string": str, "integer": int}


def refusal(tool, arguments):
    schema = tool["inputSchema"]
    for name in schema.get("required", []):
        if name not in arguments:
            return f"{name} is required"
    for name, value in arguments.items():
        expected = schema["properties"].get(name, {}).get("type")
        if expected and not isinstance(value, TYPES[expected]):
            return f"{name} must be a {expected}"
    return None


for line in sys.stdin:
    request = json.loads(line)
    if "id" not in request:
        continue
    reply = {"jsonrpc": "2.0", "id": request["id"]}
    method = request.get("method")
    params = request.get("params") or {}
    if method == "initialize":
        reply["result"] = {
            "protocolVersion": "2025-06-18",
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "docs", "version": "1"},
        }
    elif method == "ping":
        reply["result"] = {}
    elif method == "tools/list":
        reply["result"] = {"tools": TOOLS}
    elif method == "tools/call":
        tool = next((tool for tool in TOOLS if tool["name"] == params.get("name")), None)
        if tool is None:
            reply["error"] = {"code": -32602, "message": "unknown tool"}
        else:
            problem = refusal(tool, params.get("arguments") or {})
            text = problem or "page"
            reply["result"] = {"content": [{"type": "text", "text": text}], "isError": problem is not None}
    else:
        reply["error"] = {"code": -32601, "message": "method not found"}
    print(json.dumps(reply), flush=True)

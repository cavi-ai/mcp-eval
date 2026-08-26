#!/usr/bin/env python3
"""Pagination and latency fixture. argv[1] selects the catalog defect:
clean | duplicate | invalid | stalled. The `slow_read` tool sleeps so
latency-budget cases have a deterministic over-budget signal."""
import json
import sys
import time

mode = sys.argv[1] if len(sys.argv) > 1 else "clean"

PAGE_ONE = [
    {"name": "alpha_read", "description": "a", "inputSchema": {"type": "object", "properties": {}}},
    {"name": "beta_status", "description": "b", "inputSchema": {"type": "object", "properties": {}}},
    {"name": "slow_read", "description": "s", "inputSchema": {"type": "object", "properties": {}}},
]
PAGE_TWO = [
    {"name": "gamma_reset", "description": "g", "inputSchema": {"type": "object", "properties": {}}},
    {"name": "delta_slow", "description": "d", "inputSchema": {"type": "object", "properties": {}}},
]
if mode == "duplicate":
    PAGE_TWO = [dict(PAGE_ONE[0])] + PAGE_TWO
if mode == "invalid":
    PAGE_TWO[0] = {"name": "broken_tool", "description": "x"}

for line in sys.stdin:
    request = json.loads(line)
    method = request.get("method")
    if method == "notifications/initialized":
        continue
    if method == "initialize":
        result = {
            "protocolVersion": "2025-06-18",
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "paged-fixture", "version": "1"},
        }
    elif method == "tools/list":
        cursor = request["params"].get("cursor") if isinstance(request.get("params"), dict) else None
        if cursor is None:
            tools, next_cursor = PAGE_ONE, "page-2"
        else:
            tools, next_cursor = PAGE_TWO, None
            if mode == "stalled":
                # Never-ending cursor whose later pages are empty: exercises
                # the stall bound without tripping the duplicate check.
                tools, next_cursor = ([] if cursor == "page-2" else PAGE_TWO), "page-3"
            if mode == "invalid":
                tools = [{"name": "schemaless", "description": "s"}]
        result = {"tools": tools}
        if next_cursor is not None:
            result["nextCursor"] = next_cursor
    elif method == "tools/call":
        tool = request["params"]["name"]
        if tool in ("slow_read", "delta_slow"):
            time.sleep(0.15)
        result = {
            "content": [{"type": "text", "text": "CANARY raw response"}],
            "structuredContent": {"status": "ready"},
        }
    else:
        result = {}
    sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": result}) + "\n")
    sys.stdout.flush()

"""Bounded rejection, cancellation races, and error-data fixture."""
import json
import sys

mode = sys.argv[1]
calls = 0
late_id = None


def emit(request_id, **body):
    print(json.dumps({"jsonrpc": "2.0", "id": request_id, **body}), flush=True)


for line in sys.stdin:
    request = json.loads(line)
    method = request.get("method")
    if "id" not in request:
        continue
    request_id = request["id"]
    if late_id is not None:
        emit(late_id, result={"content": []})
        late_id = None
    if method == "initialize":
        emit(request_id, result={"protocolVersion": "2025-06-18", "capabilities": {"tools": {}}})
    elif method == "tools/list":
        emit(request_id, result={"tools": [{"name": "read", "inputSchema": {"type": "object"}, "annotations": {"readOnlyHint": True}}]})
    elif method == "tools/call":
        calls += 1
        arguments = request["params"]["arguments"]
        if len(arguments.get("blob", "")) >= 65536:
            error = {"code": -32600, "message": "Invalid Request"}
            if mode == "null-application-error":
                error["code"] = -32000
            if mode == "parse-reject":
                error["code"] = -32700
            if mode == "null-malformed-error":
                del error["message"]
            emit(request_id + 1 if mode == "wrong-id" else None, error=error)
            if mode == "reject-exit":
                sys.exit(0)
        elif mode.startswith("retry-"):
            error = {"code": -32602, "message": "Invalid params"}
            if mode != "retry-missing":
                error["data"] = {"retryable": False}
            if mode == "retry-conflict":
                error["retryable"] = True
            emit(request_id, error=error)
        elif mode == "cancel-late" and calls == 2:
            late_id = request_id
        elif (mode == "cancel-error" and calls == 2) or (mode == "cancel-recovery-error" and calls == 3):
            emit(request_id, error={"code": -32000, "message": "Tool failed"})
        elif mode == "cancel-tool-error" and calls == 2:
            emit(request_id, result={"isError": True, "content": []})
        else:
            emit(request_id, result={"content": []})
            if mode == "cancel-exit" and calls == 2:
                sys.exit(0)
    else:
        emit(request_id, result={"_meta": {}} if mode == "reject-meta" else {})

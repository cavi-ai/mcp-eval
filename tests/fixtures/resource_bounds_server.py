"""Exercise evaluator transport bounds using real subprocess pipes."""
import json
import os
import sys
import time

mode = sys.argv[1]
if mode == "no-read":
    time.sleep(10)
    sys.exit(0)
for raw in sys.stdin:
    message = json.loads(raw)
    if "id" not in message:
        continue
    if mode == "oversized":
        sys.stdout.write("CANARY" + "x" * (4 * 1024 * 1024) + "\n")
        sys.stdout.flush()
        time.sleep(10)
    elif mode == "interleaved":
        end = time.monotonic() + 2
        while time.monotonic() < end:
            print(json.dumps({"jsonrpc": "2.0", "method": "notifications/progress", "params": {}}), flush=True)
            time.sleep(0.01)
    elif mode == "notifications":
        for _ in range(140):
            print(json.dumps({"jsonrpc": "2.0", "method": "notifications/progress", "params": {}}), flush=True)
        print(json.dumps({"jsonrpc": "2.0", "id": message["id"], "result": {}}), flush=True)
    elif mode == "pid":
        print(json.dumps({"jsonrpc": "2.0", "id": message["id"], "result": {"pid": os.getpid()}}), flush=True)

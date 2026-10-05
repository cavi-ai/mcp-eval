"""Test adapter: evaluate the same HTTP MCP service through stdio framing."""
import json
import sys
from urllib.request import Request, urlopen

for line in sys.stdin:
    message = json.loads(line)
    request = Request(
        sys.argv[1], data=json.dumps(message).encode(),
        headers={"Content-Type": "application/json"}, method="POST",
    )
    with urlopen(request, timeout=5) as response:
        body = response.read()
    if "id" in message:
        print(body.decode(), flush=True)

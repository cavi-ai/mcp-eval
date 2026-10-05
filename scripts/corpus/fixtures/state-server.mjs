// Real stdio MCP fixture with a separately hosted, mutable backing service.
import { createInterface } from "node:readline";
const tools = [{ name: "read_state", description: "Read the fixture dataset", annotations: { readOnlyHint: true }, inputSchema: { type: "object", properties: {}, additionalProperties: false } }];
for await (const line of createInterface({ input: process.stdin })) {
  const request = JSON.parse(line);
  if (request.id === undefined) continue;
  let result;
  if (request.method === "initialize") result = { protocolVersion: "2025-06-18", capabilities: { tools: {} }, serverInfo: { name: "state-fixture", version: "1" } };
  else if (request.method === "tools/list") result = { tools };
  else if (request.method === "tools/call") {
    if (request.params?.name !== "read_state") {
      console.log(JSON.stringify({ jsonrpc: "2.0", id: request.id, error: { code: -32602, message: "Unknown tool" } }));
      continue;
    }
    const response = await fetch(process.env.FIXTURE_BACKEND_URL, { headers: { "x-fixture-tool": "read_state" }, signal: AbortSignal.timeout(5000) });
    if (!response.ok) throw new Error("fixture backend unavailable");
    const state = await response.json();
    result = { content: [{ type: "text", text: JSON.stringify(state) }], structuredContent: state };
  } else result = {};
  console.log(JSON.stringify({ jsonrpc: "2.0", id: request.id, result }));
}

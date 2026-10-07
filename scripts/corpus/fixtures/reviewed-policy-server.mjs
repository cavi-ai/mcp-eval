import { createInterface } from "node:readline";
import { writeFileSync } from "node:fs";
const schema = { type: "object", properties: {}, additionalProperties: false };
const tools = [
  { name: "read_legacy", inputSchema: schema },
  { name: "annotated_writer", annotations: { readOnlyHint: false }, inputSchema: schema },
  { name: "unreviewed_write", inputSchema: schema },
];
for await (const line of createInterface({ input: process.stdin })) {
  const request = JSON.parse(line);
  if (request.id === undefined) continue;
  let result;
  if (request.method === "initialize") result = { protocolVersion: "2025-06-18", capabilities: { tools: {} }, serverInfo: { name: "reviewed-policy-fixture", version: "1" } };
  else if (request.method === "tools/list") result = { tools };
  else if (request.method === "tools/call") {
    if (!tools.some(tool => tool.name === request.params?.name)) {
      console.log(JSON.stringify({ jsonrpc: "2.0", id: request.id, error: { code: -32602, message: "Unknown tool" } }));
      continue;
    }
    if (request.params.name !== "read_legacy") writeFileSync(process.env.FIXTURE_MUTATION_MARKER, "writer was called");
    const args = request.params.arguments;
    const invalid = !args || typeof args !== "object" || Array.isArray(args) || Object.keys(args).length !== 0;
    result = invalid ? { content: [{ type: "text", text: "Invalid arguments" }], isError: true }
      : { content: [{ type: "text", text: '{"answer":42}' }], structuredContent: { answer: 42 } };
  } else result = {};
  console.log(JSON.stringify({ jsonrpc: "2.0", id: request.id, result }));
}

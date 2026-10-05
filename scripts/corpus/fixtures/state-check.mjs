// Read the same fixture service as the MCP tools; never emit its payload.
import { createHash } from "node:crypto";
const response = await fetch(process.env.FIXTURE_BACKEND_URL, { signal: AbortSignal.timeout(5000) });
if (!response.ok) process.exit(1);
console.log(createHash("sha256").update(Buffer.from(await response.arrayBuffer())).digest("hex"));

# Privacy and authorization

MCP Eval is read-only by default and has no raw-payload mode. It stores structured call metadata and shaped arguments, not raw response bodies. Human error text is reduced to the constant `{message}` plus a salted template identifier; the original message is not stored.

Server labels, methods, tool names, keys, enum values, numeric and boolean values, and registrable domains are retained only within their documented bounded grammars. Other hosts and string values are reduced to privacy-safe categories or length buckets. Server stderr passes through to the client and is not journaled.

Probe records are tagged `synthetic` and use the same persistence boundary. Raw manifest arguments, response bodies, tool descriptions, sandbox descriptions, and raw errors are not stored or printed in summaries. Manifest files can still contain sensitive operational inputs and are outside the share-safe boundary.

Only `<MCPEVAL_HOME>/store/` is safe to share. The fingerprint salt lives at `<MCPEVAL_HOME>/.salt`, outside `store/`, and must never accompany it. Do not share the entire capture root.

Mutation requires two independent controls: the manifest case uses `"access": "mutating"` and names a declared sandbox, and the operator passes `--allow-mutation`. A missing or invalid manifest, undeclared sandbox, or missing flag never authorizes mutation. `generate --confirm-read-only` attests that an eligible tool is read-only and does not authorize mutation.

HTTP endpoints are loopback-only by default. Remote endpoints require HTTPS plus `--allow-remote-http`. Optional authorization is read from `MCPEVAL_HTTP_AUTHORIZATION`, validated, used in memory, and never persisted or printed. The HTTP proxy may relay an incoming `Authorization` value in memory, but it does not originate calls or grant mutation permission.

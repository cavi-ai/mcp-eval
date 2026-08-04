# mcp-eval

`mcp-eval` captures what MCP servers cost agents and turns repeated friction into
queryable evidence a development agent can act on.

Phase 1 ships the capture path: a privacy-bounded, redacting stdio shim and a
SQLite index over its JSONL journal.

## Quick start

```sh
cargo build --release
./target/release/mcpeval shim --server demo -- your-mcp-server --flags
./target/release/mcpeval index
```

Set `MCPEVAL_HOME` to choose the capture root. If it is unset, `mcp-eval` uses
`$HOME/.mcp-eval` when `HOME` is available. Set `MCPEVAL_SESSION` when records
from one shim process must correlate with an existing agent session; otherwise
the shim generates a UUID for that process.

The stdio shim targets Unix and Windows and expects newline-delimited JSON-RPC.
See [the installation guide](docs/install.md) for MCP client configuration and
live verification.

## What is recorded

For completed JSON-RPC calls and inbound server notifications, the journal keeps
timestamps, session and sequence identifiers, the configured server name, method
and tool names, latency, outcome, shim overhead, and shaped `params.arguments`
when present. It does not persist raw response bodies.

Every invalid or otherwise unparseable frame produces a content-free record with
only `ts`, `session`, `seq`, `server`, an `unparsed/{direction}` method, the
`unparsed` outcome, `shim_self_us`, and `kind`. The direction is `outbound` or
`inbound`; the frame's raw bytes are forwarded unchanged but never stored.

Argument values are reduced as follows:

- object keys, container sizes, and the shape of the first array item;
- string length buckets and a `uuid` marker;
- schema-declared enum values;
- numbers and booleans as typed constants;
- for HTTP(S) URLs, the true public-suffix registrable domain;
- `ip`, `localhost`, or `host` instead of literal IP, localhost, or
  nonregistrable host values.

Human error text is never retained: every error message becomes the constant
`{message}`. Error codes may retain scalar numbers or booleans; string codes,
layers, and kinds are reduced to length buckets.

## Privacy boundary

The configured server name, `MCPEVAL_SESSION`, method and tool names, argument
keys, schema-declared enum values, numeric and boolean arguments, and
registrable domains are intentionally queryable and therefore persisted. Do not
put secrets in those fields. Server stderr is passed through unchanged to the
client, not written to the journal. There is no verbose or raw-payload mode.

See [the Phase 1 design](docs/design/2026-08-04-mcp-eval.md) for the complete
data model and scope.

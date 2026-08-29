# Findings and verification

Indexing reads JSONL capture records into SQLite and derives failure windows. Promotion groups recurring failures by server, tool, error code, and salted template identifier:

```sh
mcpeval index
mcpeval promote
mcpeval findings --format md
```

Use `--threshold <number>` for a one-run promotion threshold override. It takes precedence over `promotion_threshold` in `<MCPEVAL_HOME>/config.json`; thresholds must be finite and non-negative. An issue still needs evidence from two distinct sessions before promotion.

`findings` supports `agent`, `md`, and `json`. Every format contains sanitized identifiers, aggregate metrics, and already-shaped arguments—not raw error templates, annotation notes, sessions, salt, or raw argument values.

Add a bounded human observation to a captured call with:

```sh
mcpeval annotate --session session-token --seq 4 \
  --kind workaround --note "Used the documented alternate tool"
```

The note is the deliberate prose channel in the store: it is limited to 240 characters and cannot contain control characters, but those checks do not redact its content. Never put credentials, private paths, customer identifiers, or raw payload fragments in `--note`. Use only one of the annotation kinds accepted by the binary, and manually review or remove notes before sharing store records.

To advance a finding lifecycle, select exactly one matching manifest case:

```sh
mcpeval verify --finding finding-0123456789abcdef \
  --case literal-status --manifest mcp-eval.manifest.json \
  -- your-mcp-server --flags
```

The first green result moves an open finding to `verifying`; the third consecutive green closes it. A red result resets the streak and reopens a verifying or closed finding. Findings without an attached probe remain open, require manual closure, and are capped at medium severity.

## Serving findings and the agent loop

`mcpeval serve` exposes a loopback Streamable HTTP MCP endpoint so agents can consume evaluation data natively:

```sh
mcpeval serve --listen 127.0.0.1:8091
```

The surface offers three read-only data tools — `list_findings`, `get_finding`, and `get_readiness_trends` — plus two agent-loop tools:

- `run_probe` executes the deterministic battery against any server using an inline manifest and returns the full `mcpeval.probe-report/v1` document with per-case verdicts, measurements, and remediation hints. Mutation is never authorized through this tool: no argument combination can enable sandboxed or mutating cases.
- `scaffold` introspects a live server's catalog and returns a starter manifest JSON without writing files; `confirm_read_only` attests the candidate tools are read-only, exactly as with `mcpeval init`.

Together they close the loop inside the agent's own protocol: scaffold a manifest, probe the server under development, read structured verdicts and fixes, and re-run until green. The endpoint is loopback-only, serves only share-safe content, and never persists anything.

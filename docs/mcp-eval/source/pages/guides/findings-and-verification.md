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

The note is the deliberate prose channel in the store: it is limited to 240 characters and cannot contain control characters. Use only one of the annotation kinds accepted by the binary.

To advance a finding lifecycle, select exactly one matching manifest case:

```sh
mcpeval verify --finding finding-0123456789abcdef \
  --case literal-status --manifest mcp-eval.manifest.json \
  -- your-mcp-server --flags
```

The first green result moves an open finding to `verifying`; the third consecutive green closes it. A red result resets the streak and reopens a verifying or closed finding. Findings without an attached probe remain open, require manual closure, and are capped at medium severity.

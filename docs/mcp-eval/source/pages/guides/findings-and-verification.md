# Findings and verification

Indexing reads JSONL capture records into SQLite and derives failure windows. Promotion groups recurring failures from captured calls (never `mcpeval probe` calls) by server, tool, and salted template identifier:

```sh
mcpeval index
mcpeval promote
mcpeval findings --format md
```

A finding keeps every distinct error code of its group in `err_codes` and the most frequent in `err_code`. Each finding carries a defect class and a one-line server-side fix hint; when several classes apply, the first in this order wins:

| Class | Evidence |
| --- | --- |
| `unstable-error-code` | the group returned more than one error code |
| `false-success` | a failure carries a `false-success` annotation |
| `blocked-optimal-path` | a failure carries a `blocked-optimal-path` annotation |
| `recovers-on-retry` | every failure is retryable and, after at least one, a call to the same tool within the next three calls of the session succeeded |
| `retry-did-not-recover` | every failure is retryable and no such recovery was captured |
| `recurring-error` | none of the above |

Finding IDs from earlier releases, which included the error code, re-key once on the next `mcpeval promote`: the most recently updated lifecycle state and its probe history move to the new ID, and older duplicates are dropped.

`mcpeval promote` prints `promoted F of I issues`, followed by how many issues were seen in one session only and how many scored below the threshold when either count is nonzero. When there are no promoted findings, `mcpeval findings` writes a one-line explanation to stderr and exits 0.

Use `--threshold <number>` for a one-run promotion threshold override. It takes precedence over `promotion_threshold` in `<MCPEVAL_HOME>/config.json`; thresholds must be finite and non-negative. An issue still needs evidence from two distinct sessions before promotion.

`findings` supports `agent`, `md`, and `json`. Every format contains sanitized identifiers, aggregate metrics, the defect class and fix hint, and already-shaped arguments—not raw error templates, annotation notes, sessions, salt, or raw argument values.

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
mcpeval serve --listen 127.0.0.1:8091 --allow-spawn
```

The surface offers three read-only data tools — `list_findings`, `get_finding`, and `get_readiness_trends` — plus one write-side tool and, with `--allow-spawn`, two agent-loop tools that launch the server process the agent names:

- `run_probe` executes the deterministic battery against any server using an inline manifest and returns the full `mcpeval.probe-report/v1` document with per-case verdicts, measurements, and remediation hints. Mutation is never authorized through this tool: no argument combination can enable sandboxed or mutating cases.
- `scaffold` introspects a live server and returns the same starter manifest JSON as `mcpeval init`, without writing files: catalog budgets, pagination, protocol negotiation, and surface listing when declared; schema-guessability, degradation-over-n, latency-budget, and output-schema cases per candidate tool answering `{}` plus one contention and one payload-bounds case. Candidates are zero-required tools annotated `readOnlyHint: true`; `confirm_read_only` attests that unannotated zero-required tools are read-only and adds them. Tools annotated `destructiveHint: true` or `readOnlyHint: false` are never called.
- `record_annotation` records an agent-authored observation about a captured call, identified by `(session, seq)`: the same fixed kind set and 240-character bounded, control-character-free note as `mcpeval annotate`, with the session hashed before persistence. This is the one deliberate prose channel in the store — the tool builds, validates, and stores the identical record the CLI command does, and the annotation notes still require a human pass before a store is shared.

Together they close the loop inside the agent's own protocol: scaffold a manifest, probe the server under development, read structured verdicts and fixes, record what the agent observed, and re-run until green. The endpoint is loopback-only and serves only share-safe content. It refuses any request whose `Host` is not loopback (403, or 400 when missing), whose `Origin` is present and not loopback (403), or whose `Content-Type` is not `application/json` (415), so a web page cannot reach it through the browser, directly or through DNS rebinding.

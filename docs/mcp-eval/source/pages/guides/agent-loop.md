# Agent loop over MCP

`mcpeval serve` exposes evaluation data and tools on a loopback Streamable HTTP
MCP endpoint, so a coding agent can scaffold a manifest, probe the server it is
editing, read structured verdicts and fixes, record what it observed, and re-run
until green, without leaving its own protocol.

```sh
mcpeval serve --listen 127.0.0.1:8091
mcpeval serve --listen 127.0.0.1:8091 --allow-spawn
```

`--print-config` prints an MCP client config snippet for the endpoint and exits
without serving.

## Tools

| Tool | Available | Does |
| --- | --- | --- |
| `list_findings` | always | Promoted findings, optionally filtered by lifecycle state |
| `get_finding` | always | One finding by `finding-*` identifier, with its repro |
| `get_readiness_trends` | always | Readiness history per server, oldest first |
| `record_annotation` | always | Records an agent-authored observation about a captured call |
| `run_probe` | `--allow-spawn` | Runs an inline manifest against a server |
| `scaffold` | `--allow-spawn` | Returns a starter manifest for a live server |
| `score` | `--allow-spawn` | Measures standard readiness without a manifest |
| `verify_finding` | `--allow-spawn` | Verifies a finding with one manifest case |

The four evaluation tools launch the server process the agent names, so they
are listed and callable only with `--allow-spawn`.

### Data tools

`list_findings`, `get_finding`, and `get_readiness_trends` serve only what
`mcpeval findings --format json` and `mcpeval trends` would print. They are
annotated read-only and idempotent. `list_findings` returns no findings before
the first `mcpeval index`; a corrupt or incomplete index is a tool error.

`record_annotation` takes an `event_id` or unambiguous legacy `(session, seq)`
coordinates, a kind from the fixed set, and a note of at most 240 characters
with no control characters. It builds, validates, and stores the same record as
`mcpeval annotate`, with legacy sessions hashed before persistence. The note is
the one deliberate prose channel in the store and still needs a human pass
before a store is shared. The tool is annotated as a non-idempotent local write.

### Evaluation tools

- `run_probe` runs the deterministic battery against any server with an inline
  manifest and returns the full `mcpeval.probe-report/v2` document: per-case
  verdicts, measurements, and remediation hints. No argument combination
  enables sandboxed or mutating cases.
- `scaffold` returns the same starter manifest JSON as `mcpeval init` without
  writing files. Candidates are zero-required tools annotated
  `readOnlyHint: true`; `confirm_read_only` attests that unannotated
  zero-required tools are read-only and adds them. Tools annotated
  `destructiveHint: true` or `readOnlyHint: false` are never called. See
  [Scaffold a manifest](capture-and-probe.md#scaffold-a-manifest) for the cases
  it writes.
- `score` runs the same standard battery as `mcpeval score`, without a
  manifest. It takes `command` or `url`, an optional `server_label`,
  `confirm_read_only`, and `skip_tools`, and returns the standard report with a
  text summary. An unavailable target reports readiness as unmeasured.
- `verify_finding` takes `finding_id`, `case_id`, an inline `manifest`, and
  `command` or `url`. It runs exactly one matching read-only case through the
  same verification service as `mcpeval verify` and returns `verified`,
  `reason`, `lifecycle` (state and consecutive passes), and `report`. A
  mid-case transport failure returns `lifecycle: null`; a startup failure
  returns a tool error. Neither records verification evidence. Red outcomes
  reset the streak and reopen findings under the usual
  [lifecycle rules](findings-and-verification.md#verification).

The evaluation tools never authorize mutation or remote HTTP. `score` and
`verify_finding` reject unknown arguments, including attempted authorization
overrides. The four evaluation tools carry conservative destructive and
open-world hints; hints describe behavior and never replace `--allow-spawn` or
grant mutation permission.

## Results and errors

- Every tool advertises an output schema, and results carry conforming
  structured content alongside text. The evaluation tools advertise the
  report's top-level fields; `mcpeval schema report` prints the complete schema.
- Tool failures return `isError` results.
- A call naming no listed tool, or with arguments violating a schema-validated
  tool's input schema, answers JSON-RPC `-32602`.
- The service answers `ping`.

## Capacity and deadlines

- Four HTTP workers keep data queries responsive during an evaluation.
- Only one evaluation runs at a time; an overlapping request receives an error
  to retry after it finishes.
- At most sixteen connections wait for a worker; excess connections are closed.
- Headers and bodies share a five-second request deadline.

Every probe, score, verification, and scaffold operation shares a five-minute
deadline and a 4096-request cap across transports, catalog pages, reconnects,
and concurrent calls. The gate and standard battery use the same budget.
Handshakes and discovery consume requests; notifications and server-request
replies only consume time. Shorter operation timeouts still apply.

Exhaustion reports `evaluation-budget-exceeded`, retains completed case
verdicts, leaves readiness unmeasured, and gives unfinished verification no
credit. Startup and scaffold exhaustion return an error. Local parsing,
journaling, reporting, and cleanup can add time beyond the server-interaction
deadline.

## Browser isolation

The endpoint is loopback-only and serves only share-safe content. A web page
cannot reach it through the browser, directly or through DNS rebinding, because
it refuses:

- a repeated `Host`, `Origin`, or `Content-Type` (400);
- a non-loopback `Origin` (403);
- a missing `Host` (400) or a non-loopback `Host` (403);
- any method other than POST (405);
- a request without `Content-Type: application/json` (415).

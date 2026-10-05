# Capture and probe

## Capture stdio traffic

Place the server command after `--` so MCP Eval can proxy it without interpreting server flags:

```sh
mcpeval shim --server demo -- your-mcp-server --flags
```

The stdio shim expects newline-delimited JSON-RPC. It forwards invalid frames unchanged but records only a content-free `unparsed/outbound` or `unparsed/inbound` event.

## Capture Streamable HTTP traffic

Start a loopback proxy and configure the MCP client to use its listen URL:

```sh
mcpeval shim-http --server demo \
  --listen 127.0.0.1:8090 \
  --upstream http://127.0.0.1:8080/mcp
```

Remote upstreams require HTTPS and `--allow-remote-http`. Endpoint URLs cannot contain credentials, query strings, or fragments, and redirects are disabled.

## Capture and event identity

Every recorder instance and probe run mints a random UUIDv4
`identity.capture_id`; every journal event has a distinct `identity.event_id`.
Sequence numbers order recorded events within that capture. They are not
JSON-RPC request IDs, and a capture ID does not create another logical session
for the two-session promotion requirement. Reconnects within a probe run retain
its capture identity.

Indexing builds failure windows within one capture. Historical records have no
recorder IDs; indexing retains them without inventing provenance. Their windows
stay within one session/server pair and are omitted if that pair has duplicate
sequence numbers. Unique legacy coordinates cannot prove recorder continuity.

A copied modern event counts once when its complete sanitized metadata agrees.
An event ID with conflicting metadata, two events claiming one capture sequence,
or a capture ID spanning sessions or servers rejects the rebuild without
replacing the previous index. Legacy duplicates remain separate observations.
These UUIDs identify local evidence; they do not authenticate its source.

## Run probes

Run all manifest cases over stdio, or select one dimension with `--probe`:

```sh
mcpeval probe --server demo --manifest mcp-eval.manifest.json \
  --probe discovery-cost -- your-mcp-server --flags
```

Use `--url` for a Streamable HTTP endpoint:

```sh
mcpeval probe --server demo --manifest mcp-eval.manifest.json \
  --url http://127.0.0.1:8080/mcp
```

Use `--format json` when a deterministic, versioned result is needed for a CI
artifact or baseline:

```sh
mcpeval probe --server demo --manifest mcp-eval.manifest.json \
  --format json --probe token-cost -- your-mcp-server --flags
```

The JSON schema is `mcpeval.probe-report/v2` (`mcpeval schema report`). It
contains only the generator, the validated server label, the manifest's
SHA-256, case, probe, and tool identifiers, fixed failure labels with their
remediation hints and declared bounds, pass state, numeric measurements, the
gate counts, and the standard readiness object (area scores and lost checks).
It has no timestamps, sessions, arguments, responses, descriptions, schemas,
or raw error text.

Mutating cases are rejected unless each case names a declared sandbox and the invocation includes `--allow-mutation`. The flag authorizes only manifest-declared cases; it does not turn capture into a mutation capability.

`init` writes discovery-cost and token-cost budgets from the measured
catalog, pagination, protocol negotiation, and surface listing when
`initialize` declares `resources` or `prompts`. Per-tool cases come from the
candidates: zero-required tools the server annotates `readOnlyHint: true`,
plus, with `--confirm-read-only` (an operator attestation that unannotated
tools are read-only), the unannotated zero-required tools. A tool annotated
`destructiveHint: true` or `readOnlyHint: false` is never called, with or
without the attestation. `init` calls up to 20 candidates with `{}` and, for
each that succeeds, adds schema-guessability, degradation-over-n,
latency-budget (bound from the measured latency), and output-schema when an
`outputSchema` is declared, plus one contention and one 1 MB payload-bounds
case. `--tool <NAME>` (repeatable) restricts the candidates; a name the
catalog lacks, or one init cannot call (annotated as a writer, required
arguments, or unattested), is a usage error (exit 2).
`--dry-run` prints each tool's decision after `initialize` and `tools/list`
and calls no tool, writes no file, and skips the `--output` existence check. Error honesty, state
recovery, instruction fidelity, cancellation, sampling, elicitation, resource
subscription, and completion cases need inputs `init` cannot infer and are
never scaffolded.

For a promoted finding with a valid tool, `generate` writes a deterministic one-case read-only manifest: a `degradation-over-n` case whose `max_attempts` is sized from the finding's observed failure rate to catch the defect with 95% probability (3 for a deterministic error, up to 100), so the probe passes once the call succeeds. Arguments are rebuilt from the recorded shape: enum members, numbers, booleans, and nulls are kept; strings and UUIDs become `""` and the nil UUID, and arrays become `[]`. The command prints the probe ID, `probe=<kind> max_attempts=<n>`, and one `fill: <path> (<shape>)` line per placeholder to complete before `mcpeval verify`:

```sh
mcpeval generate --finding finding-0123456789abcdef \
  --output generated.manifest.json --confirm-read-only
```

`--confirm-read-only` is an operator attestation about the selected tool, not mutation authorization. Add `--force` only when replacing the requested output file is intentional.

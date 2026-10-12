# Capture and probe

## Capture stdio traffic

Place the server command after `--` so MCP Eval can proxy it without interpreting server flags:

```sh
mcpeval shim --server demo -- your-mcp-server --flags
```

The stdio shim runs on Unix and Windows; other targets return an unsupported-target
error. It expects newline-delimited JSON-RPC, inherits the server's environment
unchanged, and passes the server's stderr through untouched. It forwards invalid
frames unchanged but records only a content-free `unparsed/outbound` or
`unparsed/inbound` event.

To capture an MCP client's sessions, wrap the server command in the client's
config. Before:

```json
{ "command": "/path/to/server", "args": ["serve"] }
```

After:

```json
{
  "command": "/path/to/mcpeval",
  "args": ["shim", "--server", "myserver", "--", "/path/to/server", "serve"]
}
```

On Windows, use the `.exe` binaries and escaped backslashes:

```json
{
  "command": "C:\\path\\to\\mcpeval.exe",
  "args": ["shim", "--server", "myserver", "--", "C:\\path\\to\\server.exe", "serve"]
}
```

Set `MCPEVAL_HOME` in the client environment to choose the capture root. On
Windows, set it explicitly when the client provides no `HOME`. Set
`MCPEVAL_SESSION` to correlate captures with an agent session; when it is unset,
the shim generates a UUID once per process.

## Verify a live capture

1. Restart the MCP client so it starts the server through the shim.
2. Call one clearly read-only tool.
3. Run `mcpeval index` with the same `MCPEVAL_HOME`; it must report a nonzero
   call count.
4. Inspect grouped counts:

   ```sh
   sqlite3 "$MCPEVAL_HOME/index.db" \
     "SELECT server, tool, outcome, COUNT(*) FROM calls GROUP BY 1,2,3 ORDER BY 4 DESC;"
   ```

5. Run `mcpeval doctor --check-redaction`. It must exit 0 with no findings and
   name the salt path as a must-not-share item. A finding names a file and line,
   never the matched text, and is a redaction bug: stop capturing and inspect
   that line.

## Capture Streamable HTTP traffic

Start a loopback proxy and configure the MCP client to use its listen URL:

```sh
mcpeval shim-http --server demo \
  --listen 127.0.0.1:8090 \
  --upstream http://127.0.0.1:8080/mcp
```

Remote upstreams require HTTPS and `--allow-remote-http`. Endpoint URLs cannot contain credentials, query strings, or fragments, and redirects are disabled.

The proxy listens only on an explicit loopback socket and bounds JSON and SSE
responses. Before forwarding anything upstream, it refuses:

- a repeated `Host`, `Origin`, or `Content-Type` (400);
- a non-loopback `Origin` (403);
- a missing `Host` (400) or a non-loopback `Host` (403);
- a POST without `Content-Type: application/json` (415).

## Capture and event identity

Every recorder instance and probe run mints a random UUIDv4
`identity.capture_id`; every journal event has a distinct `identity.event_id`.
Sequence numbers order recorded events within that capture. They are not
JSON-RPC request IDs, and a capture ID does not create another logical session
for the two-session promotion requirement. Reconnects within a probe run retain
its capture identity. `MCPEVAL_SESSION` names the logical session; it is stored
as a stable `session:<sha256>` token, never as the raw value.

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

The HTTP client accepts `application/json` and `text/event-stream` responses,
carries the negotiated `Mcp-Session-Id`, and sends `MCP-Protocol-Version`.
Redirects are disabled, responses are bounded to 8 MiB, and network timeouts are
five seconds. Mutation gates apply identically to stdio and HTTP.

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

## Scaffold a manifest

`init` introspects a live server and writes every case that needs no domain
input. Server-level cases come first:

- `discovery-cost` and `token-cost` budgets from the measured catalog;
- `pagination` with `max_pages` 5;
- `protocol-negotiation`;
- `surface-listing` when `initialize` declares `resources` or `prompts`.

Candidates are zero-required tools annotated `readOnlyHint: true`. With
`--confirm-read-only`, an operator attestation that unannotated tools are
read-only, unannotated zero-required tools are candidates too. A tool annotated
`destructiveHint: true` or `readOnlyHint: false` is never called.

`init` calls up to 20 candidates with `{}`. Each one that succeeds gets:

- `schema-guessability`;
- `degradation-over-n` with 5 attempts;
- `latency-budget` with 3 attempts, bounded at four times the measured latency,
  rounded up to 100 ms and kept between 1 and 60 seconds;
- `output-schema` when the tool declares an `outputSchema`.

The first such tool also gets `contention`. One gets `payload-bounds`: a 1 MB
string in its first string property, else in a `payload` field. A clean
rejection passes.

`--tool <NAME>` (repeatable) restricts the candidates. A name the catalog
lacks, or one `init` cannot call (a writer, required arguments, or unattested),
is a usage error (exit 2).

`--dry-run` stops after `initialize` and `tools/list`, calls no tool, writes no
file, and skips the `--output` existence check. It prints one decision per tool:
`candidate (readOnlyHint)`, `candidate (attested)`, `needs --confirm-read-only`,
`skipped: destructiveHint`, `skipped: readOnlyHint=false`,
`skipped: required arguments`, or `skipped: not in --tool`.

Discovery must finish within 20 `tools/list` pages. When a later page fails or
the twentieth page still has a cursor, `init` exits 3 before calling tools or
writing a manifest, even with `--force`. On the same catalog, `score` leaves
readiness unmeasured. `probe` reports ordinary cases as transport errors with
zero attempts, so `verify` grants no passing credit. Pagination cases still run
to diagnose the listing; `--probe pagination` runs that diagnostic alone.

Some cases are never scaffolded; add them by hand:

- `workflow`, `error-honesty`, `state-recovery`, and `instruction-fidelity`
  need expected inputs;
- `cancellation` needs a deliberately slow tool;
- `sampling`, `elicitation`, `resource-subscription`, and `completion` need
  declared references.

## Generate a case for a finding

For a promoted finding with a valid tool, `generate` writes a deterministic
one-case read-only manifest chosen from the finding's class. Recurring and
retry failures use `degradation-over-n`, unstable codes use `error-honesty`
when retryability is recorded, and semantic findings need an explicit
`--expect` file. See [finding-specific generation](findings-and-verification.md#finding-specific-generation)
for the contracts and an expectation example.

Arguments are rebuilt from the recorded shape: enum members, numbers, booleans,
and nulls are kept; strings and UUIDs become `""` and the nil UUID, and arrays
become `[]`. The command prints the probe ID, `probe=<kind>` (with
`max_attempts=<n>` for repeated probes), and one `fill: <path> (<shape>)` line
per placeholder to complete before `mcpeval verify`:

```sh
mcpeval generate --finding finding-0123456789abcdef \
  --output generated.manifest.json --confirm-read-only
```

`--confirm-read-only` is an operator attestation about the selected tool, not mutation authorization. Add `--force` only when replacing the requested output file is intentional.

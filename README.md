# mcp-eval

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

`mcp-eval` captures what MCP servers cost agents and turns repeated friction into
queryable evidence a development agent can act on.

The deterministic battery includes `discovery-cost`, `schema-guessability`,
`error-honesty`, `state-recovery`, and `contention`. A finding closes only after three consecutive green
verification runs and reopens automatically on regression, without discarding its
probe history.

## Quick start

```sh
cargo build --release
./target/release/mcpeval shim --server demo -- your-mcp-server --flags
./target/release/mcpeval shim-http --server demo \
  --listen 127.0.0.1:8090 --upstream http://127.0.0.1:8080/mcp
./target/release/mcpeval index
./target/release/mcpeval promote
./target/release/mcpeval findings --format agent
./target/release/mcpeval generate --finding finding-0123456789abcdef \
  --confirm-read-only \
  --output generated.manifest.json
./target/release/mcpeval probe --server demo \
  --manifest mcp-eval.manifest.json -- your-mcp-server --flags
./target/release/mcpeval probe --server demo \
  --manifest mcp-eval.manifest.json --url http://127.0.0.1:8080/mcp
./target/release/mcpeval verify --finding finding-0123456789abcdef \
  --case literal-status --manifest mcp-eval.manifest.json \
  -- your-mcp-server --flags
```

Set `MCPEVAL_HOME` to choose the capture root. If it is unset, `mcp-eval` uses
`$HOME/.mcp-eval` when `HOME` is available. `MCPEVAL_SESSION` is transformed to
a stable `session:<sha256>` token before persistence; otherwise the shim hashes
a generated UUID for that process.

The stdio shim targets Unix and Windows and expects newline-delimited JSON-RPC.
`shim-http` provides the same privacy-safe capture boundary for Streamable HTTP
POST traffic. It accepts connections only on an explicit loopback socket, forwards
to a validated endpoint, preserves MCP protocol and session headers, and supports
bounded JSON and finite SSE responses. The probe runner also supports MCP
Streamable HTTP endpoints with JSON or SSE responses.
See [the installation guide](docs/install.md) for MCP client configuration and
live verification.

Probe manifests are strict, versioned JSON. Unknown fields are rejected. A
`mutating` case must name a declared sandbox and the operator must also pass
`--allow-mutation`; a missing or invalid manifest, undeclared sandbox, or missing
flag never authorizes mutation. `instruction-fidelity` currently checks declared,
machine-readable result fields, scalar values, outcomes, and error codes. It does
not send tool descriptions or results to an external LLM.
`discovery-cost` enforces declared bounds on tool count and the encoded `tools/list`
catalog size. `schema-guessability` checks that the selected tool exposes a coherent
object schema, that every required field is declared and supplied by the manifest's
naive call, and that the call succeeds. Catalog descriptions and schemas are measured
in memory but never persisted or printed.
`error-honesty` verifies stable error codes, truthful retryability metadata, and
eventual recovery within a declared bound. `state-recovery` executes an explicit
failure, recovery, and validation sequence. Its calls use the normal sanitized
synthetic-record boundary, and mutating sequences require both sandbox declaration
and `--allow-mutation`.
`contention` synchronizes two independent MCP clients against the same declared tool
and passes only when both calls succeed. Each call uses the sanitized synthetic-record
boundary.

HTTP endpoints are loopback-only by default. Remote endpoints require HTTPS plus
`--allow-remote-http`. Endpoint URLs must not contain credentials, query strings, or
fragments; redirects are disabled. Optional authorization is read from
`MCPEVAL_HTTP_AUTHORIZATION`, validated, and never persisted or printed. Responses are
bounded to 8 MiB and use five-second connect/read/write timeouts.

The HTTP capture proxy follows the same upstream URL policy and I/O bounds. It
forwards incoming `Authorization` values in memory but never stores headers, endpoint
URLs, request bodies, or response bodies. It does not originate tool calls or grant
mutation permission; it only relays traffic sent by the connected client. Configure
the MCP client to use the proxy's loopback URL while `mcpeval shim-http` is running.

Promotion groups failures by server, tool, error code, and salted template
identifier. Its score combines the 95% Wilson lower bound of the observed rate,
fourteen-day recency decay, median failure-window turns, and distinct-tool blast
radius. An issue never becomes a finding until it appears in two distinct
sessions, even with a zero threshold.

The default threshold is calibrated from the checked-in synthetic seed corpus.
Set `promotion_threshold` in `<MCPEVAL_HOME>/config.json` to configure it, or use
`mcpeval promote --threshold <number>` for a one-run override. The CLI override
takes precedence. Thresholds must be finite and non-negative.

`mcpeval findings --format agent|md|json` reports only sanitized identifiers,
aggregate metrics, and already-shaped arguments. It never emits raw error
templates, annotation notes, session identifiers, the fingerprint salt, or raw
argument values. Run `mcpeval index` again after new capture data, then rerun
`mcpeval promote` before reading refreshed findings.

Each promoted finding has a deterministic, privacy-safe `finding-*` identifier and
a lifecycle state: `open`, `fix-claimed`, `verifying`, or `closed`. Use `mcpeval
verify --finding <id> --case <manifest-case>` to run exactly one matching probe.
The first green moves a finding to `verifying`; the third consecutive green closes
it. A red verification resets the streak and reopens a verifying or closed finding.
History is append-only and survives index/promotion rebuilds. Findings without an
attached probe remain open, require manual closure, and are capped at medium
severity. Verification performs finding, case, and tool checks before launching the
server. Mutating verification still requires both a declared sandbox and
`--allow-mutation`.

`mcpeval generate --finding <id> --confirm-read-only --output <path>` creates a
deterministic manifest for an eligible promoted finding. Eligibility requires a valid
tool and exactly empty captured arguments; captured non-empty arguments are never
reconstructed. `--confirm-read-only` is an operator attestation that the tool is safe
to call without mutation, not permission to mutate it. The generated case is always a
read-only, sandbox-free `degradation-over-n` probe. Generation reads an existing index,
writes only the requested manifest file, prints only the generated probe ID, and never
replaces an existing file unless `--force` is passed.

## What is recorded

For completed JSON-RPC calls and inbound server notifications, the journal keeps
timestamps, an opaque session token and sequence identifier, the validated server
label and method, tool names that satisfy the tool-name grammar (see Privacy
boundary — independent of whether `tools/list` ever declared them), latency,
outcome, shim overhead (including JSON parsing), and shaped `params.arguments`
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

Human error text is never retained as text. Every error message is reduced to
the constant `{message}`, plus `template_id` — a lowercase-hex fingerprint of
the first 8 bytes of `SHA256(salt || 0x00 || skeleton(message))`, where
`skeleton` collapses UUIDs, quoted runs, URLs, paths, and digits out of the
message first. The message itself never enters the fingerprint's output or
the store; `template_id` exists only so that two failures sharing the same
underlying defect can be told apart from two unrelated ones, which a constant
`{message}` alone cannot do. Error codes may retain scalar numbers or
booleans, or a string that satisfies the identifier grammar (non-empty,
ASCII, starts with a letter, at most 64 bytes); every other string code, and
every string `layer` or `kind`, is reduced to a length bucket instead.

An agent may also record an `annotation`: a short, typed observation (`kind` is
one of a fixed set, e.g. `false-success`, `workaround`) tied to a `(session,
seq)` call, plus `note` — a free-text field bounded to 240 characters and
scrubbed of control characters. `note` is the one deliberate prose channel in
the store; every other field described above is structured, content-free
metadata. Annotations are written to `annotations-*.jsonl`, alongside the
`calls-*.jsonl` journal.

## Privacy boundary

Server names must be 1–128 character ASCII labels. Methods use a bounded
slash-separated label grammar. A tool name is retained whenever it satisfies
the bounded tool-name grammar (non-empty, at most 128 bytes, ASCII
alphanumeric plus `_`, `-`, `.`, or `:`) — whether or not a `tools/list`
response ever declared it, so servers that load schemas on demand still get a
usable tool dimension. A call whose tool name is prose-shaped (spaces,
slashes, and the like) uses `unlisted` instead. Argument keys, schema-declared
enum values, numeric and boolean arguments, and registrable domains remain
queryable. Server stderr is passed through unchanged to the client, not
written to the journal. There is no verbose or raw-payload mode.

Probe calls use the same persistence boundary and are tagged `synthetic`. Raw
manifest arguments, response bodies, tool descriptions, sandbox descriptions, and
raw errors are never stored or printed in summaries. Manifest files may still
contain sensitive arguments or operational details, so they are not part of the
share-safe `<MCPEVAL_HOME>/store/` boundary.

The salt that makes `template_id` non-invertible is generated once and stored
at `<MCPEVAL_HOME>/.salt` (mode 0600 on Unix) — a dotfile sibling of `store/`,
deliberately outside it. **Only `<MCPEVAL_HOME>/store/` is safe to share** or
attach to an issue; the salt must never accompany it. The skeleton space of
MCP error messages is small and guessable, so salt plus journal would let
someone recover which messages produced which fingerprint. `mcpeval doctor
--check-redaction` prints the salt path on its own line as a must-not-share
reminder every time it runs.

## Project status

`mcp-eval` is under active development. The on-disk schema, CLI, and manifest
format may change before 1.0. Review the [changelog](CHANGELOG.md) when updating.

## Contributing

Bug reports and focused pull requests are welcome. See
[CONTRIBUTING.md](CONTRIBUTING.md) for development setup, quality gates, and the
privacy requirements for fixtures and tests. Security reports should follow
[SECURITY.md](SECURITY.md).

## License

Licensed under the [MIT License](LICENSE).

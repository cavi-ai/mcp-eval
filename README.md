# mcp-eval

`mcp-eval` captures what MCP servers cost agents and turns repeated friction into
queryable evidence a development agent can act on.

Phase 2 adds issue aggregation, Wilson-bounded promotion, seed-calibrated
thresholds, and privacy-safe findings reports to the capture path.

## Quick start

```sh
cargo build --release
./target/release/mcpeval shim --server demo -- your-mcp-server --flags
./target/release/mcpeval index
./target/release/mcpeval promote
./target/release/mcpeval findings --format agent
```

Set `MCPEVAL_HOME` to choose the capture root. If it is unset, `mcp-eval` uses
`$HOME/.mcp-eval` when `HOME` is available. `MCPEVAL_SESSION` is transformed to
a stable `session:<sha256>` token before persistence; otherwise the shim hashes
a generated UUID for that process.

The stdio shim targets Unix and Windows and expects newline-delimited JSON-RPC.
See [the installation guide](docs/install.md) for MCP client configuration and
live verification.

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

The salt that makes `template_id` non-invertible is generated once and stored
at `<MCPEVAL_HOME>/.salt` (mode 0600 on Unix) — a dotfile sibling of `store/`,
deliberately outside it. **Only `<MCPEVAL_HOME>/store/` is safe to share** or
attach to an issue; the salt must never accompany it. The skeleton space of
MCP error messages is small and guessable, so salt plus journal would let
someone recover which messages produced which fingerprint. `mcpeval doctor
--check-redaction` prints the salt path on its own line as a must-not-share
reminder every time it runs.


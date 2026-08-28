# mcp-eval

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

`mcp-eval` is an agnostic evaluation tool for MCP servers, with two
complementary tracks:

1. **Deterministic benchmark battery** — run a strict, manifest-driven probe
   suite against any MCP server (stdio or Streamable HTTP) and get a
   pass/fail verdict with fixed failure reasons. No LLM in the loop, no
   network services, fully reproducible: same server, same manifest, same
   result. Built for CI gating while you develop a server.
2. **Friction capture** — run your server behind a privacy-preserving shim in
   production-like agent sessions, index what it actually costs agents, and
   promote repeated friction into queryable findings a development agent can
   act on. Findings close only after three consecutive green verification
   runs and reopen automatically on regression.

Both tracks share one privacy boundary: raw payloads, error prose, and
credentials never persist. Only `<MCPEVAL_HOME>/store/` is share-safe.

## Install

```sh
cargo build --release
```

The binary is `target/release/mcpeval` (`mcpeval.exe` on Windows). crates.io,
prebuilt binaries, npm, and Homebrew distribution land with the first tagged
release; see the [changelog](CHANGELOG.md).

## Track 1: benchmark battery

Scaffold a starter manifest from a live server, then refine it. `init`
introspects the catalog, derives generous discovery and token budgets, and
smoke-tests each candidate tool with a naive `{}` call before declaring it —
so the generated manifest passes on its first run:

```sh
mcpeval init --server demo -- your-mcp-server --flags
# wrote mcp-eval.manifest.json (7 tools, 5 schema-guessability cases)
mcpeval probe --server demo -- your-mcp-server --flags
```

Manifests are strict: unknown fields, unsupported versions, and unsafe
access/sandbox combinations are rejected before the server process starts.
For editor support, validate against the JSON Schema (print it with
`mcpeval schema`, or reference [docs/mcp-eval.manifest.schema.json](docs/mcp-eval.manifest.schema.json)
via `"$schema"`):

```json
{
  "version": 1,
  "probes": [
    {
      "id": "bounded-discovery",
      "probe": "discovery-cost",
      "access": "read_only",
      "max_tools": 10,
      "max_schema_bytes": 1000
    },
    {
      "id": "naive-status",
      "probe": "schema-guessability",
      "tool": "describe_status",
      "access": "read_only",
      "arguments": {}
    },
    {
      "id": "literal-status",
      "probe": "instruction-fidelity",
      "tool": "describe_status",
      "access": "read_only",
      "arguments": {},
      "expect": {
        "outcome": "ok",
        "required_result_fields": ["status"],
        "equals": {"status": "ready"}
      }
    }
  ]
}
```

Run every case, or select one probe kind, against a stdio command or a
Streamable HTTP endpoint:

```sh
mcpeval probe --server demo --manifest mcp-eval.manifest.json -- your-mcp-server --flags
mcpeval probe --server demo --manifest mcp-eval.manifest.json \
  --probe discovery-cost --url http://127.0.0.1:8080/mcp
```

The command exits zero only when every selected case passes. Summaries contain
case IDs, probe kinds, attempt counts, first-failure positions, and fixed
reason labels — never actual arguments, responses, or errors.

`--format json` emits a versioned, deterministic document
(`mcpeval.probe-report/v1`): server label, per-case verdicts, fixed reason
labels, and measurement numbers — no timestamps, sessions, or payloads, so it
is safe to commit as a baseline or attach to CI artifacts.

```sh
mcpeval probe --server demo --manifest mcp-eval.manifest.json \
  --format json -- your-mcp-server --flags
```

`--format json` emits a versioned, deterministic document
(`mcpeval.probe-report/v1`): server label, per-case verdicts, fixed reason
labels, measurement numbers, and the readiness score — no timestamps,
sessions, or payloads, so it is safe to commit as a baseline or attach to CI
artifacts.

```sh
mcpeval probe --server demo --manifest mcp-eval.manifest.json \
  --format json -- your-mcp-server --flags
```

`--format markdown` renders the same verdicts as a pull-request-ready report
with a readiness score and badge:

```sh
mcpeval probe --server demo --manifest mcp-eval.manifest.json \
  --format markdown -- your-mcp-server --flags
```

The **readiness score** (0–100) is a deterministic composite over four
weighted categories — discovery (discovery-cost, token-cost, pagination),
reliability (degradation-over-n, error-honesty, state-recovery,
latency-budget), contract (schema-guessability, instruction-fidelity), and
concurrency (contention). Only categories present in the manifest are scored,
so partial manifests are never penalized for probes they did not declare.
The same score drives the badge URL embedded in the markdown report; no
payload or server detail ever leaves the report.

Every full-battery run appends a content-free score record to
`<MCPEVAL_HOME>/store/probes/history.jsonl`; `mcpeval trends` renders the
per-server history with score deltas.

The deterministic battery:

| Probe | What it checks |
| --- | --- |
| `discovery-cost` | Tool count and encoded `tools/list` catalog size stay within declared bounds |
| `token-cost` | Estimated context cost of the catalog — total and optional per-tool token budgets, using a deterministic model-independent estimator |
| `schema-guessability` | The selected tool exposes a coherent object schema; every required field is declared, supplied by the naive call, and the call succeeds |
| `instruction-fidelity` | Declared machine-readable result fields, scalar values, outcomes, and error codes match — deterministic and structural, never sent to an external LLM |
| `degradation-over-n` | A read-only call keeps succeeding over N attempts, with the first-failure position reported |
| `error-honesty` | Stable error codes, truthful retryability metadata, recovery within a declared bound |
| `state-recovery` | An explicit failure → recovery → validation sequence, with both later calls succeeding |
| `contention` | Two synchronized independent MCP clients both succeed against the same declared tool |
| `latency-budget` | A read-only call stays within a declared `max_latency_ms` budget across N attempts; the slowest observed latency is reported |
| `pagination` | `tools/list` cursor pagination completes within `max_pages` with unique, schema-valid entries on every page |

## Comparing servers

Run one manifest against several Streamable HTTP endpoints and diff the
verdicts side by side — useful when selecting between vendor servers or
checking a deployment against your local build. Comparison is informational:
it never exits non-zero for probe failures, so it complements rather than
replaces the `probe` gate.

```sh
mcpeval compare --server demo \
  --endpoint staging=https://staging.example/mcp \
  --endpoint vendor=https://vendor.example/mcp \
  --format markdown
```

Comparison endpoints are loopback-only unless `--allow-remote-http` is
passed, and remote endpoints require HTTPS. See [the CI guide](docs/ci.md)
for gating recipes, including the composite GitHub Action.

Mutation has two independent gates: the manifest must declare a named sandbox
referenced by the case, and the operator must pass `--allow-mutation`. A
missing or invalid manifest, undeclared sandbox, or missing flag never
authorizes mutation. Catalog descriptions and schemas are measured in memory
but never persisted or printed. `token-cost` estimates tokens as encoded bytes
divided by four, rounded up — a deterministic budget unit, not a specific
model's tokenizer — over each tool's complete `tools/list` entry, and fails
when the catalog total or any single tool exceeds its declared budget.

## Track 2: friction capture

```sh
mcpeval shim --server demo -- your-mcp-server --flags
mcpeval shim-http --server demo \
  --listen 127.0.0.1:8090 --upstream http://127.0.0.1:8080/mcp
mcpeval index
mcpeval promote
mcpeval findings --format agent
mcpeval generate --finding finding-0123456789abcdef \
  --confirm-read-only \
  --output generated.manifest.json
mcpeval verify --finding finding-0123456789abcdef \
  --case literal-status --manifest mcp-eval.manifest.json \
  -- your-mcp-server --flags
```

Set `MCPEVAL_HOME` to choose the capture root (`$HOME/.mcp-eval` by default).
`MCPEVAL_SESSION` is transformed to a stable `session:<sha256>` token before
persistence.

The stdio shim targets Unix and Windows and expects newline-delimited JSON-RPC.
`shim-http` provides the same privacy-safe capture boundary for Streamable
HTTP POST traffic: explicit loopback socket only, validated upstream,
bounded JSON and finite SSE responses. See [the installation
guide](docs/install.md) for MCP client configuration and live verification.

HTTP endpoints are loopback-only by default. Remote endpoints require HTTPS
plus `--allow-remote-http`; URLs must not contain credentials, query strings,
or fragments; redirects are disabled. Optional authorization is read from
`MCPEVAL_HTTP_AUTHORIZATION`, validated, and never persisted or printed.
Responses are bounded to 8 MiB with five-second network timeouts.

Promotion groups failures by server, tool, error code, and salted template
identifier. Its score combines the 95% Wilson lower bound of the observed
rate, fourteen-day recency decay, median failure-window turns, and
distinct-tool blast radius. An issue never becomes a finding until it appears
in two distinct sessions, even with a zero threshold. The default threshold is
calibrated from the checked-in synthetic seed corpus; override it with
`promotion_threshold` in `<MCPEVAL_HOME>/config.json` or
`mcpeval promote --threshold <number>`.

Each promoted finding has a deterministic, privacy-safe `finding-*` identifier
and a lifecycle state: `open`, `fix-claimed`, `verifying`, or `closed`.
`mcpeval verify` runs exactly one matching probe; the first green moves the
finding to `verifying`, the third consecutive green closes it, and any red
resets the streak and reopens it. History is append-only and survives
index/promotion rebuilds. Findings without an attached probe remain open and
are capped at medium severity.

Findings become actionable where work happens. `export-issues` writes one
GitHub-issue-ready markdown file per open finding — evidence, severity,
shape-level repro, and the exact `generate`/`verify` commands — so the loop
can run through your issue tracker:

```sh
mcpeval export-issues --dir issues/
```

Agents can also consume the data natively: `serve` exposes findings and
readiness trends as MCP tools over a loopback Streamable HTTP endpoint, so a
coding agent can query its own friction without touching the store:

```sh
mcpeval serve --listen 127.0.0.1:8091
```

| Tool | Returns |
| --- | --- |
| `list_findings` | Sanitized finding rows (server, tool, state, severity, evidence counts), optionally filtered by lifecycle state |
| `get_finding` | One finding by `finding-*` identifier, including its shape-level repro |
| `get_readiness_trends` | Readiness-score history per server, oldest first |

## What is recorded

For completed JSON-RPC calls and inbound server notifications, the journal
keeps timestamps, an opaque session token and sequence identifier, the
validated server label and method, tool names that satisfy the tool-name
grammar, latency, outcome, shim overhead, and shaped `params.arguments` when
present. It does not persist raw response bodies. Every unparseable frame
produces a content-free record; the raw bytes are forwarded unchanged but
never stored.

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
message first. The message itself never enters the fingerprint's output or the
store; `template_id` exists only so that two failures sharing the same
underlying defect can be told apart from two unrelated ones. Error codes may
retain scalar numbers or booleans, or a string that satisfies the identifier
grammar; every other string code, and every string `layer` or `kind`, is
reduced to a length bucket instead.

An agent may also record an `annotation`: a short, typed observation (`kind`
is one of a fixed set, e.g. `false-success`, `workaround`) tied to a
`(session, seq)` call, plus `note` — a free-text field bounded to 240
characters and scrubbed of control characters. `note` is the one deliberate
prose channel in the store; every other field is structured, content-free
metadata.

## Privacy boundary

Server names must be 1–128 character ASCII labels. Methods use a bounded
slash-separated label grammar. A tool name is retained whenever it satisfies
the bounded tool-name grammar — whether or not `tools/list` ever declared it.
A call whose tool name is prose-shaped uses `unlisted` instead. Server stderr
is passed through unchanged to the client, not written to the journal. There
is no verbose or raw-payload mode.

Probe calls use the same persistence boundary and are tagged `synthetic`. Raw
manifest arguments, response bodies, tool descriptions, sandbox descriptions,
and raw errors are never stored or printed in summaries. Manifest files may
still contain sensitive arguments or operational details, so they are not part
of the share-safe `<MCPEVAL_HOME>/store/` boundary.

The salt that makes `template_id` non-invertible is generated once and stored
at `<MCPEVAL_HOME>/.salt` (mode 0600 on Unix) — a dotfile sibling of
`store/`, deliberately outside it. **Only `<MCPEVAL_HOME>/store/` is safe to
share** or attach to an issue; the salt must never accompany it.
`mcpeval doctor --check-redaction` prints the salt path on its own line as a
must-not-share reminder every time it runs.

## Project status

`mcp-eval` is under active development. The on-disk schema, CLI, and manifest
format may change before 1.0. Review the [changelog](CHANGELOG.md) when
updating.

## Contributing

Bug reports and focused pull requests are welcome. See
[CONTRIBUTING.md](CONTRIBUTING.md) for development setup, quality gates, and
the privacy requirements for fixtures and tests. Security reports should
follow [SECURITY.md](SECURITY.md).

## License

Licensed under the [MIT License](LICENSE).

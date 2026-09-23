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

Documentation: [cavi-ai.xyz/docs/mcp-eval](https://cavi-ai.xyz/docs/mcp-eval/).

## Install

```sh
brew install cavi-ai/tap/mcpeval
npm install --global @cavi-ai/mcp-eval
cargo install mcpeval --locked
```

Each installs `mcpeval` and the demo server `mcpeval-demo`. Homebrew (macOS
and Linux, x64/arm64) and npm (also Windows x64) install the prebuilt release
archive after checking its pinned SHA-256; the archives and their checksums are
also on the [releases page](https://github.com/cavi-ai/mcp-eval/releases).
Building from a checkout works too:

```sh
cargo build --release
```

The binaries are `target/release/mcpeval` and `target/release/mcpeval-demo`
(`.exe` on Windows).

## Quickstart

No server of your own yet? `mcpeval-demo` is a bundled MCP server with a
clean personality and a set of `--broken <aspect>` personalities that
reproduce specific defects (incoherent schema, unfaithful results, unstable
error codes, bloated catalogs, broken pagination, slow calls, dropped
cancellations, echoed protocol versions, malformed sampling and elicitation
requests, missing subscription notifications, malformed completions);
`mcpeval-demo --help` lists every aspect, and an unknown aspect exits 2:

```sh
mcpeval init --server demo --confirm-read-only \
  --output demo.manifest.json -- mcpeval-demo
mcpeval probe --server demo \
  --manifest demo.manifest.json -- mcpeval-demo
# readiness 100/100
mcpeval probe --server demo --manifest demo.manifest.json \
  -- mcpeval-demo --broken stalled-cursor
# pagination-stalled-cursor
```

## Track 1: benchmark battery

Scaffold a starter manifest from a live server, then refine it. `init`
introspects the server and writes every probe kind that needs no
domain input:

- always: `discovery-cost` and `token-cost` budgets derived from the
  measured catalog, `pagination` (`max_pages` 5), `protocol-negotiation`,
  and `surface-listing` when `initialize` declares `resources` or `prompts`;
- per tool, over the candidates: zero-required tools annotated
  `readOnlyHint: true`, plus, with `--confirm-read-only` (an attestation that
  unannotated tools are read-only), the unannotated zero-required tools. A
  tool annotated `destructiveHint: true` or `readOnlyHint: false` is never
  called. `init` calls up to 20 candidates with `{}` and, for each one that
  succeeds, writes `schema-guessability`, `degradation-over-n` (5 attempts),
  `latency-budget` (3 attempts, four times the measured latency rounded up
  to 100 ms, 1–60 s), and `output-schema` when the tool declares an
  `outputSchema`; the first such tool also gets `contention`, and one gets
  `payload-bounds` (a 1 MB string in its first string property, else a
  `payload` field; a clean rejection passes).

`--tool <NAME>` (repeatable) restricts the candidates to the named tools; a
name the catalog lacks, or one init cannot call (annotated as a writer,
required arguments, or unattested), exits 2.
`--dry-run` stops after `initialize` and `tools/list` and prints each tool's
decision (`candidate (readOnlyHint)`, `candidate (attested)`,
`needs --confirm-read-only`, `skipped: destructiveHint`,
`skipped: readOnlyHint=false`, `skipped: required arguments`,
`skipped: not in --tool`); it calls no tool and writes no file.

Not scaffolded: `error-honesty`, `state-recovery`, and
`instruction-fidelity` need expected inputs; `cancellation` needs a
deliberately slow tool; `sampling`, `elicitation`, `resource-subscription`,
and `completion` need declared references. Add them by hand.

```sh
mcpeval init --server demo --confirm-read-only -- your-mcp-server --flags
# wrote mcp-eval.manifest.json (7 tools, 21 cases: discovery-cost 1, token-cost 1, pagination 1, protocol-negotiation 1, schema-guessability 5, degradation-over-n 5, latency-budget 5, contention 1, payload-bounds 1)
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

`--format sarif` emits a SARIF 2.1.0 document for GitHub code scanning:
upload it (`github/codeql-action/upload-sarif` or the `code-scanning` API)
and every failing case becomes a code-scanning alert on its line in the
manifest, with its fixed reason and remediation hint. Run it from the
repository root so the manifest path resolves; each server gets its own
code-scanning category.

The command exits zero only when every selected case passes, 1 when a case
fails its probe, 2 on a usage error, and 3 when the evaluation could not
complete (a case that timed out or lost its server is reported as
`transport-*` and the report is still written). Summaries contain
case IDs, probe kinds, attempt counts, first-failure positions, and fixed
reason labels — never actual arguments, responses, or errors. Every failing
case prints a **remediation hint**: the concrete server-side fix for that
reason. A case that exceeded a manifest bound also prints
`bound=<field> limit=<n> observed=<n>` (the markdown table's `Bound`
column), and a `token-budget-exceeded` case lists its three heaviest
tools. Hints are suppressed by `--brief` for scripts, rendered in the
markdown report under *Remediation*, and always available standalone:

```sh
mcpeval explain pagination-stalled-cursor
# the cursor sequence never terminated within `max_pages`; emit no
# `nextCursor` on the final page and never re-serve a page a cursor
# already returned
mcpeval explain   # list every fixed reason
```

`--format json` emits a versioned, deterministic document
(`mcpeval.probe-report/v1`): generator version, server label, manifest
SHA-256, per-case verdicts, tool names, fixed reason labels with their
remediation hints and the declared bound that failed, measurement numbers,
and the readiness score — no timestamps, sessions, or payloads, so it is safe
to commit as a baseline or attach to CI artifacts. `mcpeval schema report`
and `mcpeval schema diff` print its JSON Schema and the diff document's.

```sh
mcpeval probe --server demo --manifest mcp-eval.manifest.json \
  --format json -- your-mcp-server --flags
```

`--format markdown` renders the same verdicts as a pull-request-ready report
with a readiness score and badge. Add `--price-per-mtok <USD>` to translate
the measured catalog cost into consequence: the catalog is charged to every
session before the first tool call, so a 2,000-token catalog at $3/Mtok is
$0.006 per session ($6 per 1,000 sessions) of pure context tax. The
estimator stays model-independent — pricing is interpretation only, and the
JSON report stays price-free so committed baselines never drift when prices
change:

```sh
mcpeval probe --server demo --manifest mcp-eval.manifest.json \
  --format markdown --price-per-mtok 3 -- your-mcp-server --flags
```

The **readiness score** (0–100) is a deterministic composite over four
weighted categories — discovery (discovery-cost, token-cost, pagination,
surface-listing), reliability (degradation-over-n, error-honesty,
state-recovery, latency-budget, payload-bounds), contract
(schema-guessability, instruction-fidelity, output-schema), and concurrency
(contention). Only categories present in the manifest are scored, so partial
manifests are never penalized for probes they did not declare. The same score
drives the badge URL embedded in the markdown report; no payload or server
detail ever leaves the report.

The score is **calibrated**: mcp-eval ships a corpus of readiness
observations from popular public MCP servers (`data/readiness-corpus.json`,
refreshed by `scripts/corpus/collect.sh`), and every text and markdown report
places your score against it. Only your cases of the battery the corpus was
collected with are scored for the comparison, and ties are counted —
*"corpus battery (discovery-cost, token-cost, pagination, surface-listing):
100/100, above 1, tied with 32, below 0 of 33 observed servers"*. When the
corpus records catalog sizes, a second line places your token-cost
measurement — *"catalog: 566 tokens over 12 tools, lighter than 23 of 33
observed servers (median 1186 tokens)"*. The shipped corpus overrides
cleanly: point a personal one at `<MCPEVAL_HOME>/corpus.json`.

**[State of MCP servers](docs/mcp-eval/source/pages/guides/state-of-mcp-servers.md)** —
the corpus is also published: how healthy are the MCP servers agents actually
use? 33 popular public servers, probed with the same battery, distribution
published with full method notes. Reproduce it locally with one script; add
your server by PR.

Every full-battery run appends a content-free score record to
`<MCPEVAL_HOME>/store/probes/history.jsonl`; `mcpeval trends` renders the
per-server history with score deltas between runs of the same manifest and
`manifest changed` where the manifest differs.

Five probes are this release's headline evaluation dimensions — discovery-cost, schema-guessability, error-honesty, state-recovery, and contention; the other fourteen are supplemental checks that score in the same four categories.

The deterministic battery:

| Probe | What it checks |
| --- | --- |
| **`discovery-cost`** | Tool count and encoded `tools/list` catalog size stay within declared bounds |
| `token-cost` | Estimated context cost of the catalog — total and optional per-tool token budgets, using a deterministic model-independent estimator |
| **`schema-guessability`** | The selected tool exposes a coherent object schema; every required field is declared, supplied by the naive call, and the call succeeds |
| `instruction-fidelity` | Declared machine-readable result fields, scalar values, outcomes, and error codes match — deterministic and structural, never sent to an external LLM |
| `degradation-over-n` | A read-only call keeps succeeding over N attempts, with the first-failure position reported |
| **`error-honesty`** | Stable error codes, truthful retryability metadata, recovery within a declared bound |
| **`state-recovery`** | An explicit failure → recovery → validation sequence, with both later calls succeeding |
| **`contention`** | Two synchronized independent MCP clients both succeed against the same declared tool |
| `latency-budget` | A read-only call stays within a declared `max_latency_ms` budget across N attempts; the slowest observed latency is reported |
| `pagination` | `tools/list` cursor pagination completes within `max_pages` with unique, schema-valid entries on every page |
| `payload-bounds` | A declared-oversize argument never crashes or hangs the server; `expect_handled` decides whether a clean rejection also counts as failure |
| `surface-listing` | Declared `resources`/`prompts` surfaces return well-formed, cursor-bounded listings; undeclared surfaces pass trivially |
| `output-schema` | A tool that declares `outputSchema` returns `structuredContent` covering the schema's required fields |
| `cancellation` | A cancelled read-only call is acknowledged: silence or a structured "Request cancelled" error for the request id — a full result or unrelated error fails the case |
| `protocol-negotiation` | Three fresh handshakes: the supported version is echoed, an unknown date-shaped version is answered with a date-shaped non-echoed version, and the version the server claims is itself echoable |
| `sampling` | A tool call under a client `sampling` capability: `sampling/createMessage` sub-requests are answered with a stub sample; more than `max_requests` per call, a malformed request, or a never-completing call fails the case |
| `elicitation` | A tool call under a client `elicitation` capability: `elicitation/create` sub-requests must carry a message and `requestedSchema`, are answered with the declared action, and stay within `max_requests` |
| `resource-subscription` | For a server declaring `resources.subscribe`: the declared URI is readable, subscribe succeeds, the trigger tool's call yields `notifications/resources/updated` within `max_wait_seconds`, and unsubscribe succeeds; undeclared support passes trivially |
| `completion` | For a server declaring the `completions` capability: one `completion/complete` request must answer `completion.values` (an array of strings) within `max_values`; a structured error naming the argument, a malformed envelope, or a flood of values fails; undeclared support passes trivially |

## Comparing servers

Run one manifest against several servers and diff the verdicts side by
side — useful when selecting between vendor servers or checking a deployment
against your local build. Targets are `--endpoint LABEL=URL` Streamable HTTP
endpoints, optionally plus one stdio command after `--` (its column is labeled
`stdio`); two or more targets are required. Comparison is informational: it
never exits non-zero for probe failures (only 3 when a case could not be
evaluated), so it complements rather than replaces the `probe` gate.

```sh
mcpeval compare --server demo \
  --endpoint staging=https://staging.example/mcp \
  --endpoint vendor=https://vendor.example/mcp \
  --format markdown

# a hosted endpoint against the local stdio demo server
mcpeval compare --server demo \
  --endpoint staging=https://staging.example/mcp \
  -- mcpeval-demo
```

Comparison endpoints are loopback-only unless `--allow-remote-http` is
passed, and remote endpoints require HTTPS. See [the CI guide](docs/ci.md)
for gating recipes, including the composite GitHub Action. The action
installs the checksum-verified release pinned by `distribution/release.json`
(or a preinstalled `mcpeval`), writes the report to the job summary, exposes
`passed`, `readiness`, `report`, and `exit-code` outputs, and optionally
gates on a committed `baseline` and uploads SARIF to code scanning. It is
self-tested in this repository on every push, consuming itself exactly as a
downstream repository would.

Reports are portable: `mcpeval report <baseline.json> --format markdown|sarif`
re-renders any committed `mcpeval.probe-report/v1` document without
re-running a server, so a probe job can run in CI, publish the JSON as an
artifact, and a separate step (or a human, later) renders the report.
`mcpeval serve --print-config` emits a ready-to-paste MCP client config for
the agent loop.

## Gating against a baseline

`mcpeval diff` is the time axis that `compare` is the space axis of: it
compares two committed `mcpeval.probe-report/v1` documents of one server —
the baseline and the current run — and classifies every case as
**regressed**, **fixed**, **changed** (still failing, for a different
reason), or **unchanged**, matching cases by id and probe kind. Readiness moves with
the verdicts; measurement movement (catalog tokens, slowest latency) is
reported per case.

```sh
mcpeval diff baseline.json report.json
# catalog-pagination         REGRESSED pagination-stalled-cursor
# readiness  100 → 85
# 1 regressed, 0 fixed, 0 changed, 3 unchanged, 0 removed, 0 added

mcpeval diff baseline.json report.json --fail-on-regression
# exits non-zero when any case regressed; fixes and manifest growth are
# informational, because adding a case is an improvement, not a drift

mcpeval diff baseline.json report.json --fail-on-change
# exits non-zero when a failing case now fails for a different reason

mcpeval diff baseline.json report.json --format json
# versioned, deterministic mcpeval.probe-diff/v1 document — safe to
# attach to CI artifacts beside the reports
```

`--format markdown` renders a pull-request-ready movement table, and `--`
in place of either path reads stdin. Both documents are share-safe report
documents by construction, so the diff carries only case IDs, verdicts,
fixed reason labels, and measurement numbers. See [the CI
guide](docs/ci.md) for the gating recipe that replaces hand-rolled `jq`
diffs.

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
bounded JSON and finite SSE responses. Like `serve`, it refuses a request
without a loopback `Host` (400 when missing or repeated, 403 otherwise),
with a non-loopback `Origin` (403), or a POST without `Content-Type:
application/json` (415), before forwarding anything upstream. See [the
installation guide](docs/install.md) for MCP client configuration and live
verification.

HTTP endpoints are loopback-only by default. Remote endpoints require HTTPS
plus `--allow-remote-http`; URLs must not contain credentials, query strings,
or fragments; redirects are disabled. Optional authorization is read from
`MCPEVAL_HTTP_AUTHORIZATION`, validated, and never persisted or printed.
Responses are bounded to 8 MiB with five-second network timeouts.

Promotion counts captured calls only, never `mcpeval probe` calls, and
groups failures by server, tool, and salted template identifier. A finding
keeps every distinct error code of its group in `err_codes` and the most
frequent in `err_code`, and carries a defect class (`unstable-error-code`,
`false-success`, `blocked-optimal-path`, `recovers-on-retry`,
`retry-did-not-recover`, or `recurring-error`) with a one-line server-side
fix hint. Finding IDs from earlier releases, which included the error code,
re-key once on the next `mcpeval promote`; the most recently updated
lifecycle state and its probe history move to the new ID. `findings --format
json` also carries `retryable`: `true` when every failure was retryable,
`false` when none was, `null` when mixed or unreported. The promotion
score combines the 95% Wilson lower bound of the observed rate, fourteen-day recency decay, median failure-window turns, and
distinct-tool blast radius. An issue never becomes a finding until it appears
in two distinct sessions, even with a zero threshold. The default threshold is
calibrated from the checked-in synthetic seed corpus; override it with
`promotion_threshold` in `<MCPEVAL_HOME>/config.json` or
`mcpeval promote --threshold <number>`.

`mcpeval generate` writes a one-case read-only manifest for a finding: a
`degradation-over-n` case whose `max_attempts` is sized from the observed
failure rate to catch the defect with 95% probability (3 for a
deterministic error, up to 100), so the probe passes once the call
succeeds. The arguments come from the recorded shape: enum members,
numbers, booleans, and nulls are kept; strings and UUIDs become
placeholders and arrays become `[]`, each printed as `fill: <path>
(<shape>)` to complete before `mcpeval verify`.

Each promoted finding has a deterministic, privacy-safe `finding-*` identifier
and a lifecycle state: `open`, `fix-claimed`, `verifying`, or `closed`.
`mcpeval verify` runs exactly one matching probe; the first green moves the
finding to `verifying`, the third consecutive green closes it, and any red
resets the streak and reopens it; a red run prints its `reason=` and the
remediation hint. History is append-only and survives
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
mcpeval serve --listen 127.0.0.1:8091 --allow-spawn   # also run_probe and scaffold
```

`run_probe` and `scaffold` launch the server process the agent names, so
they are listed only with `--allow-spawn`. Every request must carry a
loopback `Host`, a loopback `Origin` if any, and `Content-Type:
application/json`; anything else is refused with 400, 403, or 415, so a web
page cannot reach the endpoint through the browser.

| Tool | Returns |
| --- | --- |
| `list_findings` | Sanitized finding rows (server, tool, state, severity, evidence counts), optionally filtered by lifecycle state |
| `get_finding` | One finding by `finding-*` identifier, including its shape-level repro |
| `get_readiness_trends` | Readiness-score history per server, oldest first |
| `run_probe` | Execute the read-only battery against any server with an inline manifest and get the full `mcpeval.probe-report/v1` document plus remediation hints — mutation is never authorized through this surface |
| `scaffold` | Introspect a live server and return the same starter manifest JSON as `mcpeval init`, without writing files |
| `record_annotation` | Record the agent's own observation about a captured call (same fixed kinds and 240-character bounded note as `mcpeval annotate`); the session is hashed before persistence |

With `run_probe`, `scaffold` (both behind `--allow-spawn`), and `record_annotation`, the whole loop is
native MCP: the agent scaffolds a manifest, probes the server it is
editing, reads structured verdicts and fixes, re-runs, and records what it
observed along the way — without leaving its tool protocol.

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

`mcpeval share` turns that boundary into a produced artifact instead of a
hand-picked file list. It runs the redaction sweep first and **refuses to
package a store the sweep flags** (exit 1), then assembles a directory containing the
store records, a `SHARE.md` manifest of what is inside and what was
deliberately excluded (salt, `index.db`, manifests), and a loud warning when
annotation notes need manual review:

```sh
mcpeval share --dir mcpeval-envelope
# attach mcpeval-envelope/ to the issue — the salt is never in it
```

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

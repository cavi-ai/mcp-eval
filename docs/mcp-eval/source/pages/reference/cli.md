# CLI reference

MCP Eval {{PRODUCT_VERSION}} installs the `mcpeval` binary. Use
`mcpeval help <COMMAND>` for the binary's exact invocation text.

| Command | Does |
| --- | --- |
| `probe` | Runs the manifest's cases (the gate), then the standard battery for readiness |
| `score` | Runs the standard battery alone, with no manifest |
| `init` | Scaffolds a manifest from a live server |
| `guidance` | Compares server instructions with capability-dependent tool catalogs |
| `compare` | Runs one manifest against several servers side by side |
| `report` | Re-renders a committed report without re-running a server |
| `diff` | Classifies case movement between two committed reports |
| `schema` | Prints a published JSON Schema |
| `explain` | Prints the remediation hint for a fixed reason |
| `trends` | Shows readiness history from full probe runs |
| `shim` | Proxies an stdio server and captures sanitized metadata |
| `shim-http` | Proxies Streamable HTTP and captures sanitized metadata |
| `index` | Loads JSONL records and derives failure windows |
| `promote` | Aggregates indexed failures and promotes supported findings |
| `findings` | Renders sanitized findings |
| `annotate` | Records a bounded observation about a captured call |
| `generate` | Writes a one-case read-only manifest for a finding |
| `verify` | Runs one manifest case and updates the finding lifecycle |
| `export-issues` | Writes issue drafts for open findings |
| `serve` | Serves findings, trends, and agent-loop tools over loopback MCP |
| `doctor` | Runs store-hygiene checks |
| `share` | Packages the share-safe envelope |

The default manifest path for `probe`, `verify`, and `report` is
`mcp-eval.manifest.json`.

## Evaluate

### `probe`

`mcpeval probe --server <SERVER>` plus `-- <CMD>...` or `--url <URL>`.

- `--manifest <PATH>` selects the manifest.
- `--probe <KIND>` runs one probe kind and skips the standard battery.
- `--format text|json|markdown|sarif` selects the output.
- `--brief` suppresses remediation hints in text output.
- `--price-per-mtok <USD>` adds session-cost interpretation to text and
  markdown.
- `--allow-mutation` authorizes manifest-declared sandbox mutations.
- `--gate-only` skips the standard battery.
- `--confirm-read-only` attests that tools with neither `readOnlyHint` nor
  `destructiveHint` are read-only, so the standard battery calls them.
- `--skip-tool <NAME>` (repeatable) is never called by the standard battery,
  and skipping never raises the score. A name the server does not list exits 2.
- `--min-readiness <SCORE>` exits 1 when readiness is under the floor, even
  when every manifest case passed.
- `--min-area <AREA=SCORE>` (repeatable) exits 1 when that area scores under
  its floor; `AREA` is `protocol`, `catalog`, `context`, `error-honesty`,
  `reliability`, or `coverage`.
- `--exclude-latency` compares the floors with every latency check at full
  marks (`readiness.score_without_latency`); latency depends on the machine.
  A floor needs the standard battery: with `--gate-only` or `--probe` it
  exits 2.
- `--allow-remote-http` permits an explicitly selected remote HTTPS endpoint.

The five headline evaluation dimensions for `--probe` are `contention`,
`error-honesty`, `state-recovery`, `discovery-cost`, and `schema-guessability`.
The CLI also accepts the supplemental probes `token-cost`,
`degradation-over-n`, `instruction-fidelity`, `latency-budget`, `pagination`,
`payload-bounds`, `surface-listing`, `output-schema`, `cancellation`,
`protocol-negotiation`, `sampling`, `elicitation`, `resource-subscription`,
`completion`, and `workflow`; they are not additional headline dimensions in
version {{PRODUCT_VERSION}}.

### `score`

`mcpeval score --server <SERVER>` plus `-- <CMD>...` or `--url <URL>`. Runs the
standard battery with no manifest and reports readiness under the named mcpeval
standard, its areas, and every lost point. Supports
`--format text|json|markdown`, `--brief`, `--confirm-read-only`,
`--skip-tool <NAME>`, `--min-readiness <SCORE>`, `--min-area <AREA=SCORE>`,
`--exclude-latency`, and `--allow-remote-http`, as for `probe`.

### `init`

`mcpeval init --server <SERVER>` plus `-- <CMD>...` or `--url <URL>`. Scaffolds
server-level cases and per-tool cases for candidates answering `{}`; see
[Scaffold a manifest](../guides/capture-and-probe.md#scaffold-a-manifest).

- `--output <PATH>` sets the manifest path (default `mcp-eval.manifest.json`);
  `--force` replaces an existing manifest.
- `--allow-remote-http` permits an explicitly selected remote HTTPS endpoint.
- Candidates are zero-required tools annotated `readOnlyHint: true`;
  `--confirm-read-only` attests that unannotated zero-required tools are
  read-only and adds them. Tools annotated `destructiveHint: true` or
  `readOnlyHint: false` are never called.
- `--tool <NAME>` (repeatable) restricts candidates. A name the catalog lacks,
  or one `init` cannot call (a writer, required arguments, or unattested),
  exits 2.
- `--dry-run` prints each tool's decision without calling any tool or writing
  the manifest.

### `guidance`

`mcpeval guidance --server <SERVER>` plus a target. Compares instruction
references with catalogs under client capability profiles without calling
tools.

- `--format text|json` selects the output (default `text`).
- `--url` with `--allow-remote-http` follows the usual remote opt-in.
- `--profile` (repeatable) selects profiles; the default is `none`, `roots`,
  `sampling`, `elicitation`, and `all`.
- `--require-tool <NAME>` (repeatable) names a tool every selected profile must
  list; a missing tool fails the gate.
- `--settle-ms` waits 0 to 5,000 milliseconds before reading the catalog.

Prose references produce review candidates; only explicitly required tools can
fail its gate. See [Instruction coherence](../guides/instruction-coherence.md)
for scope, privacy guarantees, and incomplete-discovery behavior.

### `compare`

`mcpeval compare --server <SERVER>` plus two or more targets: one
`--endpoint <LABEL=URL>` per Streamable HTTP endpoint and optionally one stdio
command after `--`, whose column is labeled `stdio`. Compares one manifest
across servers side by side.

- `--manifest <PATH>` selects the manifest.
- `--format text|markdown|json` selects the output.
- `--gate-only` compares the manifest verdicts without the standard battery.
- `--allow-mutation` and `--allow-remote-http` work as for `probe`.

## Reports

### `report`

`mcpeval report <DOCUMENT>` (or `-` for stdin). Re-renders a committed
`mcpeval.probe-report` document (v1 or v2) as text, markdown, or SARIF without
re-running any server. Supports `--brief`, `--price-per-mtok`, and
`--manifest <PATH>` (default `mcp-eval.manifest.json`), which locates SARIF
results.

### `diff`

`mcpeval diff <BASELINE> <CURRENT>`; either may be `-` for stdin. Compares two
committed `mcpeval.probe-report` documents (v1 or v2) from the same server and
classifies each case as regressed, fixed, changed (fails for a different
reason), or unchanged. Documents naming different servers are rejected.

- `--fail-on-regression` exits non-zero for regressions.
- `--fail-on-change` exits non-zero for changed failure reasons.
- `--max-readiness-drop <POINTS>` exits non-zero when readiness fell by more
  than `POINTS` since the baseline. Readiness that is not comparable (another
  standard or measurement profile, or a v1 document) exits 2.
- `--exclude-latency` with `--max-readiness-drop` compares
  `score_without_latency` on both sides; a document without it exits 2.
- Fixes and manifest growth are informational.
- `--format text|markdown|json`; JSON is `mcpeval.probe-diff/v2`.

### `schema`

`mcpeval schema [manifest|report|diff|guidance|corpus-verification]` prints a
published JSON Schema: the strict manifest schema for editor validation (the
default), the probe report, the probe diff, the guidance report
(`mcpeval.guidance-report/v1`), or the corpus verification artifact.

### `explain`

`mcpeval explain <REASON>` prints the remediation hint for a fixed reason;
`mcpeval explain` alone lists every reason. It covers the gate's reasons; each
lost readiness check carries its own hint.

### `trends`

`mcpeval trends [--last <COUNT>]` shows privacy-safe readiness history from full
probe runs, at most `--last` runs per server (default 10).

## Capture and findings

| Command | Shape | Notes |
| --- | --- | --- |
| `shim` | `--server <SERVER> -- <CMD>...` | Proxies an stdio server |
| `shim-http` | `--server <SERVER> --listen <LISTEN> --upstream <UPSTREAM>` | `--allow-remote-http` permits an explicitly selected remote HTTPS upstream |
| `index` | `[--rebuild]` | Loads JSONL records and derives failure windows; `--rebuild` ignores cached checkpoints and reconstructs every derived row |
| `promote` | `[--threshold <THRESHOLD>]` | Promotes supported findings |
| `findings` | `[--format agent\|md\|json]` | Default format is `agent` |
| `annotate` | `--event-id <UUID>` or `--session <SESSION> --seq <SEQ>`, plus `--kind <KIND> --note <NOTE>` | Unknown or ambiguous targets stay unlinked |
| `export-issues` | `--dir <DIR> [--include-closed] [--force]` | Content-free markdown drafts for open promoted findings; `--include-closed` adds fix-claimed, verifying, and closed findings; `--force` replaces existing files |

### `generate`

`mcpeval generate --finding <FINDING> --output <OUTPUT> --confirm-read-only`
writes a class-specific one-case read-only manifest with argument placeholders.

- Recurring and retry findings use `degradation-over-n`; unstable codes use
  `error-honesty` when retryability is recorded.
- `--expect <FILE>` supplies a reviewed expectation and selects
  `instruction-fidelity`; semantic findings require it. False-success requires
  a result assertion or an expected error.
- `--force` replaces an existing output only after validation.

### `verify`

`mcpeval verify --finding <FINDING> --case <CASE>` plus `-- <CMD>...` or
`--url <URL>`. Runs one manifest case and updates the finding lifecycle.
Supports `--manifest <PATH>`, `--allow-mutation`, and `--allow-remote-http` as
for `probe`.

## Serve and share

### `serve`

`mcpeval serve --listen <LISTEN>` serves findings and trends over a loopback
Streamable HTTP MCP endpoint. `--allow-spawn` enables `run_probe`, `scaffold`,
`score`, and `verify_finding`, which launch the server process an agent names.
`--print-config` emits an MCP client config snippet for the endpoint and exits.
See [Agent loop over MCP](../guides/agent-loop.md).

### `doctor`

`mcpeval doctor [--check-redaction]` runs store-hygiene checks.

### `share`

`mcpeval share --dir <DIR>` packages the share-safe envelope: the store records
plus a `SHARE.md` manifest, after a clean redaction sweep. A flagged sweep
packages nothing and exits `1`.

- The salt, index databases, and manifests are never copied.
- Annotation prose is omitted by default; `--include-annotation-notes` includes
  it after manual review.
- `--include-probe-history` includes readiness-trend history.
- `--force` replaces the entire envelope.

## JSON report

`probe --format json` emits the versioned, deterministic
`mcpeval.probe-report/v2` document. It contains:

- the generator name and version, the validated server label, and the
  manifest's SHA-256;
- case IDs, probe names, tool names, pass state, and numeric measurements;
- fixed failure labels with their remediation hints, and the declared bound
  behind a bound-based failure (`detail`: bound, limit, observed);
- the `gate` counts, null under `score`;
- the `readiness` object: the standard's name, the surface, and each area's
  weight, score, and lost checks with fixed reason labels and hints. It is null
  under `--gate-only` or a selected probe; `readiness_error` names why when the
  standard battery could not start. `score_without_latency` is the score with
  every latency check at full marks;
- the `readiness_gate` object when a floor was declared: the floors, whether
  latency was excluded, `passed`, and each shortfall (subject, observed,
  floor). `report` exits 1 for a document whose floor failed.

It excludes timestamps, sessions, arguments, response bodies, descriptions,
schemas, and raw error text. `mcpeval schema report` and `mcpeval schema diff`
print the JSON Schemas for the report and diff documents; version 2 only gains
optional fields. `report` and `diff` also read v1 documents; `diff` marks
readiness not comparable across standards or against a v1 document.

## Exit codes

| Code | Meaning |
| --- | --- |
| `0` | The command succeeded; every selected case passed. |
| `1` | The evaluation completed and a case failed its probe, or a gate such as a readiness floor, `diff --fail-on-regression`, `diff --max-readiness-drop`, or the `share` redaction sweep fired. |
| `2` | Usage error: invalid arguments, manifest, or input document. Nothing was evaluated. |
| `3` | The evaluation could not complete: the server was unreachable, a case lost its transport, or local I/O failed. |

A case that times out, loses its connection, or breaks the protocol
mid-exchange is reported as `transport-timeout`, `transport-closed`, or
`transport-error`, and the next case starts on a fresh connection.

- `probe`, `report`, `verify`, and `compare` still emit their output and then
  exit `3`.
- Such a case counts as not passed in the readiness score.
- `verify` leaves the finding's lifecycle unchanged.
- Full-battery runs with such a case record no trend point.

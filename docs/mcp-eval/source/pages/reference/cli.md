# CLI reference

MCP Eval {{PRODUCT_VERSION}} installs the `mcpeval` binary.

| Command | Required shape | Purpose |
| --- | --- | --- |
| `shim` | `--server <SERVER> -- <CMD>...` | Proxy an stdio server and capture sanitized metadata. |
| `shim-http` | `--server <SERVER> --listen <LISTEN> --upstream <UPSTREAM>` | Proxy Streamable HTTP capture. `--allow-remote-http` permits an explicitly selected remote HTTPS upstream. |
| `probe` | `--server <SERVER>` plus `[-- <CMD>...]` or `--url <URL>` | Run the manifest's cases (the gate), then the standard battery for readiness. Supports `--manifest`, `--probe` (one kind; skips the standard battery), `--format text\|json\|markdown\|sarif`, `--price-per-mtok <USD>` for session-cost interpretation in text and markdown, `--allow-mutation`, `--gate-only` (skip the standard battery), `--confirm-read-only` (attest that tools with neither `readOnlyHint` nor `destructiveHint` are read-only, so the standard battery calls them), `--skip-tool <NAME>` (repeatable; the standard battery never calls it, and skipping never raises the score; a name the server does not list exits 2), and `--allow-remote-http`. |
| `score` | `--server <SERVER>` plus `[-- <CMD>...]` or `--url <URL>` | Run the standard battery alone, with no manifest: readiness under the named mcpeval standard, its areas, and every lost point. Supports `--format text\|json\|markdown`, `--brief`, `--confirm-read-only`, `--skip-tool <NAME>`, and `--allow-remote-http`. |
| `init` | `--server <SERVER>` plus a target | Scaffold discovery-cost, token-cost, pagination, protocol-negotiation, and (when resources or prompts are declared) surface-listing cases, plus schema-guessability, degradation-over-n, latency-budget, and output-schema cases per candidate tool answering `{}` and one contention and one payload-bounds case. Candidates are zero-required tools annotated `readOnlyHint: true`; `--confirm-read-only` attests unannotated zero-required tools are read-only and adds them. Tools annotated `destructiveHint: true` or `readOnlyHint: false` are never called. `--tool <NAME>` (repeatable) restricts candidates; a name the catalog lacks, or one init cannot call (annotated as a writer, required arguments, or unattested), exits 2. `--dry-run` prints each tool's decision without calling any tool or writing the manifest. |
| `schema` | optional `manifest\|report\|diff` | Print a published JSON Schema: the strict manifest schema for editor validation (default), `mcpeval.probe-report/v2`, or `mcpeval.probe-diff/v2`. |
| `compare` | `--server <SERVER>` plus two or more targets: `--endpoint <LABEL=URL>` per Streamable HTTP endpoint and optionally one stdio command after `--` | Compare one manifest across servers side by side; the stdio column is labeled `stdio`. Supports text, Markdown, and JSON output. `--gate-only` compares the manifest verdicts without the standard battery. |
| `diff` | `<BASELINE> <CURRENT>` (either may be `-` for stdin) | Compare two committed `mcpeval.probe-report` documents (v1 or v2) from the same server and classify each case as regressed, fixed, changed (fails for a different reason), or unchanged. `--fail-on-regression` exits non-zero for regressions; `--fail-on-change` exits non-zero for changed failure reasons; fixes and manifest growth are informational. Documents naming different servers are rejected. Supports `--format text\|markdown\|json` (`mcpeval.probe-diff/v1`). |
| `export-issues` | `--dir <DIR>` | Write content-free markdown issue drafts for open promoted findings. |
| `trends` | optional `--last <COUNT>` | Show privacy-safe readiness history from full probe runs. |
| `report` | `<DOCUMENT>` (or `-` for stdin) | Re-render a committed `mcpeval.probe-report` document (v1 or v2) as text, markdown, or SARIF without re-running any server. Supports `--brief`, `--price-per-mtok`, and `--manifest <PATH>` (default `mcp-eval.manifest.json`), which locates SARIF results. |
| `verify` | `--finding <FINDING> --case <CASE>` plus a target | Run one manifest case and update the finding lifecycle. Supports the same manifest and authorization flags as `probe`. |
| `index` | no options | Load JSONL records and derive failure windows. |
| `promote` | optional `--threshold <THRESHOLD>` | Aggregate indexed failures and promote supported findings. |
| `generate` | `--finding <FINDING> --output <OUTPUT> --confirm-read-only` | Generate a class-specific one-case read-only manifest with argument placeholders. Recurring/retry findings use `degradation-over-n`; unstable codes use `error-honesty` when retryability is recorded. `--expect <FILE>` supplies a reviewed expectation and selects `instruction-fidelity`; semantic findings require it. False-success requires a result assertion or an expected error. `--force` replaces an existing output only after validation. |
| `findings` | optional `--format agent\|md\|json` | Render sanitized findings; default format is `agent`. |
| `serve` | `--listen <LISTEN>` | Serve findings and trends over a loopback Streamable HTTP MCP endpoint. `--allow-spawn` enables `run_probe`, `scaffold`, `score`, and `verify_finding`, which launch the server process an agent names. `--print-config` emits an MCP client config snippet for the endpoint and exits. |
| `annotate` | `--event-id <UUID>` or `--session <SESSION> --seq <SEQ>`, plus `--kind <KIND> --note <NOTE>` | Record a bounded agent-authored observation; unknown or ambiguous targets stay unlinked. |
| `doctor` | optional `--check-redaction` | Run store-hygiene checks. |
| `share` | `--dir <DIR>` | Package the share-safe envelope: the store records plus a SHARE.md manifest, after a clean redaction sweep; a flagged sweep packages nothing and exits `1`. The salt, index databases, and manifests are never copied. Annotation prose is omitted by default. Supports `--include-probe-history`, `--include-annotation-notes` after manual review, and `--force` to replace the entire envelope. |

The default manifest path for `probe`, `verify`, and `report` is `mcp-eval.manifest.json`. The five headline evaluation dimension values for `--probe` are `contention`, `error-honesty`, `state-recovery`, `discovery-cost`, and `schema-guessability`. The CLI also accepts the supplemental probes `token-cost`, `degradation-over-n`, `instruction-fidelity`, `latency-budget`, `pagination`, `payload-bounds`, `surface-listing`, `output-schema`, `cancellation`, `protocol-negotiation`, `sampling`, `elicitation`, `resource-subscription`, and `completion`; they are not additional headline dimensions in version {{PRODUCT_VERSION}}.

`probe --format json` emits the versioned, deterministic
`mcpeval.probe-report/v2` document. It contains the generator name and
version, the validated server label, the manifest's SHA-256, case IDs, probe
names, tool names, fixed failure labels with their remediation hints, the
declared bound behind a bound-based failure (`detail`: bound, limit,
observed), pass state, numeric measurements, the `gate` counts (null under
`score`), and the `readiness` object: the standard's name, the surface, and
each area's weight, score, and lost checks with fixed reason labels and
hints (null under `--gate-only` or a selected probe; `readiness_error` names
why when the standard battery could not start). It excludes timestamps,
sessions, arguments, response bodies, descriptions, schemas, and raw error
text. `mcpeval schema report` and `mcpeval schema diff` print the JSON
Schemas for the report and diff documents; version 2 only gains optional
fields. `report` and `diff` also read v1 documents; `diff` marks readiness
not comparable across standards or against a v1 document.

## Guidance discovery

`guidance --server <SERVER>` compares instruction references with catalogs
under client capability profiles without calling tools. It supports text and
JSON output, `--url` with the usual remote opt-in, repeatable `--profile` and
`--require-tool`, and a bounded `--settle-ms` initialization window. Prose
references produce review candidates; only explicitly required tools can
fail its gate. See [instruction coherence](../guides/instruction-coherence.md)
for scope, privacy guarantees, and incomplete-discovery behavior.
`schema guidance` prints `mcpeval.guidance-report/v1`'s JSON Schema.

## Exit codes

| Code | Meaning |
| --- | --- |
| `0` | The command succeeded; every selected case passed. |
| `1` | The evaluation completed and a case failed its probe, or a gate such as `diff --fail-on-regression` or the `share` redaction sweep fired. |
| `2` | Usage error: invalid arguments, manifest, or input document. Nothing was evaluated. |
| `3` | The evaluation could not complete: the server was unreachable, a case lost its transport, or local I/O failed. |

A case that times out, loses its connection, or breaks the protocol mid-exchange is reported with a `transport-timeout`, `transport-closed`, or `transport-error` reason, and the next case starts on a fresh connection. `probe`, `report`, `verify`, and `compare` still emit their output and then exit `3`. Such a case counts as not passed in the readiness score. `verify` leaves the finding's lifecycle unchanged, and full-battery runs with such a case record no trend point. `mcpeval explain` lists every reason.

Use `mcpeval help <COMMAND>` for the binary's exact invocation text.

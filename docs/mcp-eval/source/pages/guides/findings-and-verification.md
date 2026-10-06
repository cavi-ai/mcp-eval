# Findings and verification

Indexing reads JSONL capture records into SQLite and derives error failure windows. Promotion groups recurring errors from captured calls (never `mcpeval probe` calls) by server, tool, and salted template identifier. It also counts successful calls with linked `false-success` annotations as a separate group for each server and tool:

```sh
mcpeval index
mcpeval promote
mcpeval findings --format md
```

A finding keeps every distinct error code of its group in `err_codes` and the most frequent in `err_code`. Each finding carries a defect class and a one-line server-side fix hint; when several classes apply, the first in this order wins:

| Class | Evidence |
| --- | --- |
| `unstable-error-code` | the group returned more than one error code |
| `false-success` | a captured call carries a linked `false-success` annotation, including a call that returned success |
| `blocked-optimal-path` | a failure carries a `blocked-optimal-path` annotation |
| `recovers-on-retry` | every failure is retryable and, after at least one, a call to the same tool within the next three recorded calls of its failure window succeeded |
| `retry-did-not-recover` | every failure is retryable and no such recovery was captured |
| `recurring-error` | none of the above |

Finding IDs from earlier releases, which included the error code, re-key once on the next `mcpeval promote`: the most recently updated lifecycle state moves to the new ID. Older duplicate state rows are retired; every historical verification is retained.

Annotate a successful call when you observed that its result or effect contradicted the intended contract. That observation can become a finding after it recurs in two distinct captured sessions and meets the score threshold. Ordinary successes, other annotation kinds, and unknown or ambiguous annotation targets cannot create this group. Repeated annotations on one call count only once. Annotation prose is never used for grouping or exported in findings.

Successful calls annotated this way have separate finding IDs and lifecycle history from structured errors, including errors without a template. Their `err_code` and `retryable` are null and `err_codes` is empty. The rate counts annotated calls divided by all captured calls to that server and tool; it does not estimate unannotated semantic failures. Because these calls have no error failure window, their cost is one observed call and their blast radius is the affected tool. Review the intended contract explicitly with `generate --expect` before verifying a repair.

`mcpeval promote` prints `promoted F of I issues`, followed by how many issues were seen in one session only and how many scored below the threshold when either count is nonzero. When there are no promoted findings, `mcpeval findings` writes a one-line explanation to stderr and exits 0.

Use `--threshold <number>` for a one-run promotion threshold override. It takes precedence over `promotion_threshold` in `<MCPEVAL_HOME>/config.json`; thresholds must be finite and non-negative. An issue still needs evidence from two distinct sessions before promotion.

`findings` supports `agent`, `md`, and `json`. Every format contains sanitized identifiers, aggregate metrics, the defect class and fix hint, and already-shaped arguments—not raw error templates, annotation notes, sessions, salt, or raw argument values. `json` also carries `retryable`: `true` when every failure of the group was retryable, `false` when none was, and `null` when mixed or unreported.

Add a bounded human observation to a captured call with:

```sh
mcpeval annotate --event-id <identity.event_id> \
  --kind workaround --note "Used the documented alternate tool"
```

Copy the UUID from the call's `identity.event_id` in the sanitized journal.
Legacy calls accept `--session session-token --seq 4` instead. Supply exactly
one target form. A legacy reference links only when the coordinates identify
one indexed call across all servers and captures. Unknown or ambiguous targets
remain stored but unlinked and cannot change a finding's classification.
Rebuilding the index resolves targets again, including annotations recorded
before their target call was available.

The note is the deliberate prose channel in the store: it is limited to 240 characters and cannot contain control characters, but those checks do not redact its content. Never put credentials, private paths, customer identifiers, or raw payload fragments in `--note`. Use only one of the annotation kinds accepted by the binary, and manually review or remove notes before sharing store records.

To advance a finding lifecycle, select exactly one matching manifest case:

```sh
mcpeval verify --finding finding-0123456789abcdef \
  --case literal-status --manifest mcp-eval.manifest.json \
  -- your-mcp-server --flags
```

`mcpeval generate --finding <id> --confirm-read-only --output <file>` writes a
one-case manifest with the finding ID as the case ID. The probe depends on the
finding's class, as described below. Fill every argument placeholder it lists
before verifying.

The first green result moves an open finding to `verifying`; the third consecutive green closes it. A red result resets the streak and reopens a verifying or closed finding; its line ends with `reason=<reason>`, followed by an indented `hint:` line with the remediation. Findings without an attached probe remain open, require manual closure, and are capped at medium severity.

Pass credit is bound to the selected case definition, manifest version, timeout,
referenced sandbox declaration, target command or endpoint configuration, and
evaluator executable fingerprint. Changing any of those starts a fresh streak.
Formatting, object-key order, and unrelated cases do not change the binding.
The loaded definition is also the one executed; editing the manifest during a
run cannot change its verification identity. Transport failures still record
no verification evidence.

A present non-boolean `isError` in a tool result is malformed; missing remains
equivalent to false. Probes report `transport-error` and add no verification
evidence for that case. Passive capture forwards the response unchanged and
records `outcome: unknown`, with no invented error metadata. The malformed
flag and result payload never appear in diagnostics or the journal.

The authoritative state and history live in `<MCPEVAL_HOME>/lifecycle.db`.
`index.db` contains derived query tables; deleting it and running `index` then
`promote` restores the lifecycle view from the durable database. Back up
`lifecycle.db` together with `.salt`; do not include either in a share envelope.
Existing index-only evidence is imported once, retaining its original outcomes
and timestamps. Legacy evidence has no definition binding, so its passes do not
count toward a new streak. Each new run has a unique ID: replaying it cannot add
pass credit, and conflicting results under the same ID are refused.

The binding is a salted local fingerprint. Raw arguments, sandbox prose, target
paths, and endpoint values are not stored in the lifecycle database.

## Finding-specific generation

| Finding class | Default generated probe | What passing means |
| --- | --- | --- |
| `recurring-error`, `recovers-on-retry`, `retry-did-not-recover` | `degradation-over-n` | Every attempt succeeds, including the first; any error fails. |
| `unstable-error-code` with consistent recorded retryability | `error-honesty` | Error codes and retryability agree across repeated failures. A retryable failure must recover within the attempt limit. A non-retryable error must repeat consistently twice. Success on the first attempt fails this error contract. |
| `false-success`, `blocked-optimal-path` | Requires `--expect <FILE>` | The explicitly reviewed outcome and result assertions match. |
| `unstable-error-code` without recorded retryability | Requires `--expect <FILE>` | The explicitly reviewed expectation matches; retryability is not inferred. |

`--expect` is available for any class and selects `instruction-fidelity`, which
makes one call. For example, create `expect.json` with:

```json
{
  "outcome": "ok",
  "equals_paths": {"/structuredContent/status": "ready"}
}
```

Then generate the manifest:

```sh
mcpeval generate --finding finding-0123456789abcdef \
  --confirm-read-only --expect expect.json --output generated.manifest.json
```

The file uses the manifest's existing `expect` contract: `outcome` is `ok` or
`error`; successful expectations can include `required_result_fields` and
scalar `equals` for literal top-level keys, or `required_result_paths` and
scalar `equals_paths` for nested fields using JSON Pointer. See the
[manifest contract](../reference/manifest.md) for path restrictions, escaping,
and null handling. Errors can include a numeric `error_code`. Files are
limited to 64 KiB, reject unknown properties, and undergo manifest validation
before the output is opened. Invalid input never replaces an existing manifest,
even with `--force`.

False-success requires a result assertion or an expected error: checking only
`outcome: ok` would miss the reported defect. Choose assertions that actually
distinguish the bug from the repair. Captured metadata contains no result
payload or intended effect, so generation cannot invent that oracle. A result
assertion also cannot prove an external state change; author a separate
validation workflow when that is the property under test.

This minimum also applies to `verify` and MCP `verify_finding`, including
handwritten manifests. A false-success finding requires an
`instruction-fidelity` expectation, or at least one `workflow` step, with a
result assertion or expected error. Other probes and success-only expectations
are refused before server startup and leave verification history unchanged.
The oracle can be in a later workflow step that checks the affected state.
This checks that an oracle exists; review whether it actually distinguishes
the reported defect from the repair.

The supplied oracle is deliberate operator input and is copied into the local
manifest, not derived from captured prose. Use share-safe values; successful
string equalities must be identifiers, and nested equality values are refused.
Generation does not call the server, record the oracle in the capture journal,
or authorize mutation. `--confirm-read-only` remains required.

Repeated-probe attempt counts are sized from the observed failure rate toward
95% observation probability under independent trials, with a minimum of 3.
Degradation is capped at 100 attempts and error honesty at 20. The cap can
prevent reaching that target; correlated failures, early recovery, and the
two-error non-retryable contract further limit what a run observes. No
statistical detection guarantee is made for a single-call expectation.

## Serving findings and the agent loop

`mcpeval serve` exposes a loopback Streamable HTTP MCP endpoint so agents can consume evaluation data natively:

```sh
mcpeval serve --listen 127.0.0.1:8091
mcpeval serve --listen 127.0.0.1:8091 --allow-spawn
```

The surface offers three read-only data tools — `list_findings`, `get_finding`, and `get_readiness_trends` — plus annotation recording and, with `--allow-spawn`, four evaluation tools that launch the server process the agent names:

- `run_probe` executes the deterministic battery against any server using an inline manifest and returns the full `mcpeval.probe-report/v2` document with per-case verdicts, measurements, and remediation hints. Mutation is never authorized through this tool: no argument combination can enable sandboxed or mutating cases.
- `scaffold` introspects a live server and returns the same starter manifest JSON as `mcpeval init`, without writing files: catalog budgets, pagination, protocol negotiation, and surface listing when declared; schema-guessability, degradation-over-n, latency-budget, and output-schema cases per candidate tool answering `{}` plus one contention and one payload-bounds case. Candidates are zero-required tools annotated `readOnlyHint: true`; `confirm_read_only` attests that unannotated zero-required tools are read-only and adds them. Tools annotated `destructiveHint: true` or `readOnlyHint: false` are never called.
- `record_annotation` records an agent-authored observation about a captured call, identified by `event_id` or unambiguous legacy `(session, seq)` coordinates: the same fixed kind set and 240-character bounded, control-character-free note as `mcpeval annotate`, with legacy sessions hashed before persistence. This is the one deliberate prose channel in the store — the tool builds, validates, and stores the identical record the CLI command does, and the annotation notes still require a human pass before a store is shared.
- `score` uses the same standard battery as `mcpeval score`, without a manifest. Supply `command` or `url`, an optional `server_label`, `confirm_read_only`, and `skip_tools`. It returns the standard report with readiness and a text summary; an unavailable target reports readiness as unmeasured.
- `verify_finding` accepts `finding_id`, `case_id`, an inline `manifest`, and `command` or `url`. It runs exactly one matching read-only case through the same verification service as the CLI. The structured result contains `verified`, `reason`, `lifecycle` (state and consecutive passes), and `report`. A mid-case transport failure returns `lifecycle: null`; startup failure returns a tool error. Neither records verification evidence. Red outcomes reset the streak and reopen findings under the existing lifecycle rules.

The evaluation tools never authorize mutation or remote HTTP. `score` and `verify_finding` reject unknown arguments, including attempted authorization overrides. Four HTTP workers keep data queries responsive during an evaluation; only one evaluation runs at a time, and overlapping requests receive an error to retry after it finishes. At most sixteen connections wait for a worker; excess connections are closed. Headers and bodies share a five-second request deadline. Every probe, score, verification, and scaffold operation shares a five-minute deadline and a 4096-request cap across transports, catalog pages, reconnects, and concurrent calls. The gate and standard battery use the same budget. Handshakes and discovery consume requests; notifications and server-request replies only consume time. Shorter operation timeouts still apply. Exhaustion reports `evaluation-budget-exceeded`, retains completed case verdicts, leaves readiness unmeasured, and gives unfinished verification no credit. Startup and scaffold exhaustion return an error. Local parsing, journaling, reporting, and cleanup can add time beyond the server-interaction deadline.

The service supports `ping` and advertises output schemas with conforming structured results alongside existing text. The three data-query tools are annotated read-only and idempotent. Process-launching tools carry conservative destructive and open-world hints, and annotation recording is marked as a non-idempotent local write. These hints describe behavior; they never replace `--allow-spawn` or grant mutation permission.

Together they close the loop inside the agent's own protocol: scaffold a manifest, probe the server under development, read structured verdicts and fixes, record what the agent observed, and re-run until green. The endpoint is loopback-only and serves only share-safe content. It refuses any request whose `Host` is not loopback (403, or 400 when missing), whose `Origin` is present and not loopback (403), or whose `Content-Type` is not `application/json` (415), so a web page cannot reach it through the browser, directly or through DNS rebinding.

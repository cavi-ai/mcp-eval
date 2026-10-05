# Findings and verification

Indexing reads JSONL capture records into SQLite and derives failure windows. Promotion groups recurring failures from captured calls (never `mcpeval probe` calls) by server, tool, and salted template identifier:

```sh
mcpeval index
mcpeval promote
mcpeval findings --format md
```

A finding keeps every distinct error code of its group in `err_codes` and the most frequent in `err_code`. Each finding carries a defect class and a one-line server-side fix hint; when several classes apply, the first in this order wins:

| Class | Evidence |
| --- | --- |
| `unstable-error-code` | the group returned more than one error code |
| `false-success` | a failure carries a `false-success` annotation |
| `blocked-optimal-path` | a failure carries a `blocked-optimal-path` annotation |
| `recovers-on-retry` | every failure is retryable and, after at least one, a call to the same tool within the next three calls of the session succeeded |
| `retry-did-not-recover` | every failure is retryable and no such recovery was captured |
| `recurring-error` | none of the above |

Finding IDs from earlier releases, which included the error code, re-key once on the next `mcpeval promote`: the most recently updated lifecycle state moves to the new ID. Older duplicate state rows are retired; every historical verification is retained.

`mcpeval promote` prints `promoted F of I issues`, followed by how many issues were seen in one session only and how many scored below the threshold when either count is nonzero. When there are no promoted findings, `mcpeval findings` writes a one-line explanation to stderr and exits 0.

Use `--threshold <number>` for a one-run promotion threshold override. It takes precedence over `promotion_threshold` in `<MCPEVAL_HOME>/config.json`; thresholds must be finite and non-negative. An issue still needs evidence from two distinct sessions before promotion.

`findings` supports `agent`, `md`, and `json`. Every format contains sanitized identifiers, aggregate metrics, the defect class and fix hint, and already-shaped arguments—not raw error templates, annotation notes, sessions, salt, or raw argument values. `json` also carries `retryable`: `true` when every failure of the group was retryable, `false` when none was, and `null` when mixed or unreported.

Add a bounded human observation to a captured call with:

```sh
mcpeval annotate --session session-token --seq 4 \
  --kind workaround --note "Used the documented alternate tool"
```

The note is the deliberate prose channel in the store: it is limited to 240 characters and cannot contain control characters, but those checks do not redact its content. Never put credentials, private paths, customer identifiers, or raw payload fragments in `--note`. Use only one of the annotation kinds accepted by the binary, and manually review or remove notes before sharing store records.

To advance a finding lifecycle, select exactly one matching manifest case:

```sh
mcpeval verify --finding finding-0123456789abcdef \
  --case literal-status --manifest mcp-eval.manifest.json \
  -- your-mcp-server --flags
```

`mcpeval generate --finding <id> --confirm-read-only --output <file>` writes a one-case manifest for the finding, with the finding ID as the case ID: a `degradation-over-n` case whose attempts are sized from the observed failure rate to catch the defect with 95% probability (3 for a deterministic error, up to 100), so the probe passes once the call succeeds. Fill every placeholder it lists before verifying.

The first green result moves an open finding to `verifying`; the third consecutive green closes it. A red result resets the streak and reopens a verifying or closed finding; its line ends with `reason=<reason>`, followed by an indented `hint:` line with the remediation. Findings without an attached probe remain open, require manual closure, and are capped at medium severity.

Pass credit is bound to the selected case definition, manifest version, timeout,
referenced sandbox declaration, target command or endpoint configuration, and
evaluator executable fingerprint. Changing any of those starts a fresh streak.
Formatting, object-key order, and unrelated cases do not change the binding.
The loaded definition is also the one executed; editing the manifest during a
run cannot change its verification identity. Transport failures still record
no verification evidence.

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

## Serving findings and the agent loop

`mcpeval serve` exposes a loopback Streamable HTTP MCP endpoint so agents can consume evaluation data natively:

```sh
mcpeval serve --listen 127.0.0.1:8091
mcpeval serve --listen 127.0.0.1:8091 --allow-spawn
```

The surface offers three read-only data tools — `list_findings`, `get_finding`, and `get_readiness_trends` — plus annotation recording and, with `--allow-spawn`, four evaluation tools that launch the server process the agent names:

- `run_probe` executes the deterministic battery against any server using an inline manifest and returns the full `mcpeval.probe-report/v2` document with per-case verdicts, measurements, and remediation hints. Mutation is never authorized through this tool: no argument combination can enable sandboxed or mutating cases.
- `scaffold` introspects a live server and returns the same starter manifest JSON as `mcpeval init`, without writing files: catalog budgets, pagination, protocol negotiation, and surface listing when declared; schema-guessability, degradation-over-n, latency-budget, and output-schema cases per candidate tool answering `{}` plus one contention and one payload-bounds case. Candidates are zero-required tools annotated `readOnlyHint: true`; `confirm_read_only` attests that unannotated zero-required tools are read-only and adds them. Tools annotated `destructiveHint: true` or `readOnlyHint: false` are never called.
- `record_annotation` records an agent-authored observation about a captured call, identified by `(session, seq)`: the same fixed kind set and 240-character bounded, control-character-free note as `mcpeval annotate`, with the session hashed before persistence. This is the one deliberate prose channel in the store — the tool builds, validates, and stores the identical record the CLI command does, and the annotation notes still require a human pass before a store is shared.
- `score` uses the same standard battery as `mcpeval score`, without a manifest. Supply `command` or `url`, an optional `server_label`, `confirm_read_only`, and `skip_tools`. It returns the standard report with readiness and a text summary; an unavailable target reports readiness as unmeasured.
- `verify_finding` accepts `finding_id`, `case_id`, an inline `manifest`, and `command` or `url`. It runs exactly one matching read-only case through the same verification service as the CLI. The structured result contains `verified`, `reason`, `lifecycle` (state and consecutive passes), and `report`. A mid-case transport failure returns `lifecycle: null`; startup failure returns a tool error. Neither records verification evidence. Red outcomes reset the streak and reopen findings under the existing lifecycle rules.

The evaluation tools never authorize mutation or remote HTTP. `score` and `verify_finding` reject unknown arguments, including attempted authorization overrides. Four HTTP workers keep data queries responsive during an evaluation; only one evaluation runs at a time, and overlapping requests receive an error to retry after it finishes. At most sixteen connections wait for a worker; excess connections are closed. Headers and bodies share a five-second request deadline. Every probe, score, verification, and scaffold operation shares a five-minute deadline and a 4096-request cap across transports, catalog pages, reconnects, and concurrent calls. The gate and standard battery use the same budget. Handshakes and discovery consume requests; notifications and server-request replies only consume time. Shorter operation timeouts still apply. Exhaustion reports `evaluation-budget-exceeded`, retains completed case verdicts, leaves readiness unmeasured, and gives unfinished verification no credit. Startup and scaffold exhaustion return an error. Local parsing, journaling, reporting, and cleanup can add time beyond the server-interaction deadline.

The service supports `ping` and advertises output schemas with conforming structured results alongside existing text. The three data-query tools are annotated read-only and idempotent. Process-launching tools carry conservative destructive and open-world hints, and annotation recording is marked as a non-idempotent local write. These hints describe behavior; they never replace `--allow-spawn` or grant mutation permission.

Together they close the loop inside the agent's own protocol: scaffold a manifest, probe the server under development, read structured verdicts and fixes, record what the agent observed, and re-run until green. The endpoint is loopback-only and serves only share-safe content. It refuses any request whose `Host` is not loopback (403, or 400 when missing), whose `Origin` is present and not loopback (403), or whose `Content-Type` is not `application/json` (415), so a web page cannot reach it through the browser, directly or through DNS rebinding.

# Findings and verification

Captured sessions become findings in three steps: index the journals, promote
recurring failures, and read the sanitized findings. A finding closes only after
three consecutive green verification runs, and a red run reopens a verifying or
closed finding.

```sh
mcpeval index
mcpeval promote
mcpeval findings --format md
```

## Index

Indexing reads JSONL capture records into SQLite and derives error failure
windows. `index` keeps content-validated checkpoints in the private derived
database and reuses indexed calls and annotations when journals extend the
existing sorted file order.

- Changes to indexed bytes, truncation, removed files, older files, or
  earlier-file appends trigger full reconstruction.
- Identical replacement bytes can reuse the cache; file timestamps alone never
  establish identity.
- Incomplete trailing records are retried when completed.
- Complete malformed or oversized records and conflicting event identities
  reject the refresh, retaining the previous index and checkpoints.

Historical bytes are still read and hashed, so validation remains proportional
to retained journal size. When calls change, correlation windows and annotation
links are recomputed over indexed data. The checkpoints avoid reparsing and
reinserting unchanged records; they do not make history processing
constant-time. Recent readiness trends use a separate validated cache with the
same historical byte-validation cost.

Force reconstruction after repairing or replacing a derived cache:

```sh
mcpeval index --rebuild
mcpeval promote
```

Both modes replace derived findings, so rerun promotion afterward. They leave
journals and durable lifecycle evidence intact. Indexing uses shared journal
locks compatible with capture writers and checks observed bytes again before
committing. External editors that ignore locks can still change and restore
bytes between checks; use controlled, immutable journal copies when that race
matters.

## Promote

Promotion groups recurring errors from captured calls, never `mcpeval probe`
calls, by server, tool, and salted template identifier. Successful calls with
linked `false-success` annotations form a separate group for each server and
tool.

The promotion score combines four signals:

- the 95% Wilson lower bound of the observed failure rate;
- fourteen-day recency decay;
- the median number of turns in failure windows;
- the blast radius in distinct tools.

An issue becomes a finding when its score meets the threshold and it appears in
two distinct sessions. The two-session rule holds even at a zero threshold. The
default threshold is calibrated from the checked-in synthetic seed corpus. Set
your own in `<MCPEVAL_HOME>/config.json`:

```json
{ "promotion_threshold": 0.15 }
```

`--threshold <number>` overrides it for one run. Thresholds must be finite and
non-negative.

`mcpeval promote` prints `promoted F of I issues`. When either count is nonzero,
it adds how many issues were seen in one session only and how many scored below
the threshold.

## Defect classes

A finding keeps every distinct error code of its group in `err_codes` and the
most frequent in `err_code`. It carries a defect class and a one-line
server-side fix hint. When several classes apply, the first in this order wins:

| Class | Evidence |
| --- | --- |
| `unstable-error-code` | the group returned more than one error code |
| `false-success` | a captured call carries a linked `false-success` annotation, including a call that returned success |
| `blocked-optimal-path` | a failure carries a `blocked-optimal-path` annotation |
| `recovers-on-retry` | every failure is retryable and, after at least one, a call to the same tool within the next three recorded calls of its failure window succeeded |
| `retry-did-not-recover` | every failure is retryable and no such recovery was captured |
| `recurring-error` | none of the above |

Finding IDs from earlier releases, which included the error code, re-key once on
the next `mcpeval promote`: the most recently updated lifecycle state moves to
the new ID. Older duplicate state rows are retired; every historical
verification is retained.

### False-success findings

Annotate a successful call when its result or effect contradicted the intended
contract. That observation becomes a finding once it recurs in two distinct
captured sessions and meets the score threshold.

- Ordinary successes, other annotation kinds, and unknown or ambiguous
  annotation targets cannot create this group.
- Repeated annotations on one call count only once.
- Annotation prose is never used for grouping or exported in findings.

These findings have their own IDs and lifecycle history, separate from
structured errors, including errors without a template. Their `err_code` and
`retryable` are null and `err_codes` is empty. The rate counts annotated calls
over all captured calls to that server and tool; it does not estimate
unannotated semantic failures. With no error failure window, their cost is one
observed call and their blast radius is the affected tool. Review the intended
contract explicitly with `generate --expect` before verifying a repair.

## Read findings

`findings` supports `agent`, `md`, and `json`. Every format contains sanitized
identifiers, aggregate metrics, the defect class and fix hint, and
already-shaped arguments. None contains raw error templates, annotation notes,
sessions, salt, or raw argument values. `json` also carries `retryable`: `true`
when every failure of the group was retryable, `false` when none was, and `null`
when mixed or unreported.

- With a valid index and no promoted findings, `mcpeval findings` writes a
  one-line explanation to stderr and exits 0.
- When the index is missing, corrupt, or incomplete, it reports unavailable
  evidence and exits 3.
- The `mcpeval serve` findings tools report the same error for a corrupt or
  incomplete index and return no findings before the first index; see
  [Agent loop over MCP](agent-loop.md).

## Annotate calls

Add a bounded human observation to a captured call:

```sh
mcpeval annotate --event-id <identity.event_id> \
  --kind workaround --note "Used the documented alternate tool"
```

Copy the UUID from the call's `identity.event_id` in the sanitized journal.
Legacy calls accept `--session session-token --seq 4` instead; supply exactly
one target form.

- A legacy reference links only when the coordinates identify one indexed call
  across all servers and captures.
- Unknown or ambiguous targets remain stored but unlinked and cannot change a
  finding's classification.
- Rebuilding the index resolves targets again, including annotations recorded
  before their target call was available.

The note is the deliberate prose channel in the store. It is limited to 240
characters and cannot contain control characters, but those checks do not
redact its content.
Never put credentials, private paths, customer identifiers, or raw payload fragments in `--note`.
Use only one of the annotation kinds the binary accepts, and review or remove
notes before sharing store records.

## Verification

Advance a finding's lifecycle with exactly one matching manifest case:

```sh
mcpeval verify --finding finding-0123456789abcdef \
  --case literal-status --manifest mcp-eval.manifest.json \
  -- your-mcp-server --flags
```

`mcpeval generate --finding <id> --confirm-read-only --output <file>` writes a
one-case manifest with the finding ID as the case ID; see
[Finding-specific generation](#finding-specific-generation). Fill every argument
placeholder it lists before verifying.

A finding's lifecycle state is `open`, `fix-claimed`, `verifying`, or `closed`:

- A red result on an open or fix-claimed finding leaves it `fix-claimed` with no
  streak.
- The first green result moves a finding to `verifying`; the third consecutive
  green closes it.
- A red result resets the streak and reopens a verifying or closed finding. Its
  line ends with `reason=<reason>`, followed by an indented `hint:` line with the
  remediation.
- Findings without an attached probe remain open, require manual closure, and
  are capped at medium severity.

### Credit binding

Pass credit is bound to:

- the selected case definition and manifest version;
- the timeout and the referenced sandbox declaration;
- the target command or endpoint configuration;
- the evaluator executable fingerprint;
- for stdio, the bytes of the resolved executable and of regular files passed as
  arguments.

The executable is pinned before launch, and those files are checked again before
recording credit; a change during execution records no credit. Changing any
binding starts a fresh streak. Formatting, object-key order, and unrelated cases
do not change it. The loaded definition is the one executed, so editing the
manifest during a run cannot change its verification identity. Transport
failures record no verification evidence.

The binding covers observed launch artifacts, not their complete dependency
closure, environment variables, or the software and data behind an HTTP
endpoint. Use an immutable deployment and controlled service state when those
inputs matter. The binding is a salted local fingerprint: raw arguments, sandbox
prose, target paths, and endpoint values are not stored in the lifecycle
database.

### Malformed results

A tool result must be an object; null, arrays, and scalar results are malformed.
A present non-boolean `isError` is also malformed; a missing one is equivalent
to false. Probes report `transport-error` and add no verification evidence for
that case. Passive capture forwards the response unchanged and records
`outcome: unknown`, with no invented error metadata. The malformed value and
result payload never appear in diagnostics or the journal.

### Durable state

The authoritative state and history live in `<MCPEVAL_HOME>/lifecycle.db`.
`index.db` contains derived query tables: deleting it and running `index` then
`promote` restores the lifecycle view from the durable database. Back up
`lifecycle.db` together with `.salt`; do not include either in a share envelope.

Existing index-only evidence is imported once, retaining its original outcomes
and timestamps. Legacy evidence has no definition binding, so its passes do not
count toward a new streak. Each new run has a unique ID: replaying it cannot add
pass credit, and conflicting results under the same ID are refused.

## Export issues

`export-issues` writes one issue-ready markdown file per open finding: evidence,
severity, the shape-level repro, and the exact `generate` and `verify` commands.

```sh
mcpeval export-issues --dir issues/
```

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

The file uses the manifest's existing `expect` contract:

- `outcome` is `ok` or `error`.
- Successful expectations can include `required_result_fields` and scalar
  `equals` for literal top-level keys, or `required_result_paths` and scalar
  `equals_paths` for nested fields using JSON Pointer. See the
  [manifest contract](../reference/manifest.md) for path restrictions, escaping,
  and null handling.
- Errors can include a numeric `error_code`.

Files are limited to 64 KiB, reject unknown properties, and undergo manifest
validation before the output is opened. Invalid input never replaces an existing
manifest, even with `--force`.

### False-success oracles

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
The oracle can be in a later workflow step that checks the affected state. This
checks that an oracle exists; review whether it actually distinguishes the
reported defect from the repair.

The supplied oracle is deliberate operator input and is copied into the local
manifest, not derived from captured prose. Use share-safe values: successful
string equalities must be identifiers, and nested equality values are refused.
Generation does not call the server, record the oracle in the capture journal,
or authorize mutation. `--confirm-read-only` remains required.

### Attempt counts

Repeated-probe attempt counts are sized from the observed failure rate toward
95% observation probability under independent trials, with a minimum of 3.
Degradation is capped at 100 attempts and error honesty at 20. The cap can
prevent reaching that target; correlated failures, early recovery, and the
two-error non-retryable contract further limit what a run observes. No
statistical detection guarantee is made for a single-call expectation.

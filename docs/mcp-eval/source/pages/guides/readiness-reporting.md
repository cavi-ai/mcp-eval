# Readiness reporting

Every full-battery `mcpeval probe` run produces a deterministic, privacy-safe report in three formats.

## Text

The default format prints one line per case — verdict, attempts, first-failure position, fixed reason label, and measurement numbers — followed by a readiness summary line:

```text
literal-status instruction-fidelity pass attempts=1
demo readiness 87/100 discovery=2/2 reliability=1/1 contract=1/1
  corpus battery (discovery-cost, token-cost, pagination, surface-listing): 100/100, above 1, tied with 33, below 0 of 34 observed servers
```

Failing cases print a remediation hint: the concrete server-side fix for that fixed reason.

```text
p pagination fail attempts=3 first_failure=3 reason=pagination-stalled-cursor
  hint: the cursor sequence never terminated within `max_pages`; emit no
  `nextCursor` on the final page and never re-serve a page a cursor already returned
```

Use `--brief` to suppress hints in scripts. The same hints appear in the markdown report under *Remediation*, and every reason is documented standalone:

```sh
mcpeval explain pagination-stalled-cursor
mcpeval explain        # list every fixed reason
```

## JSON

`--format json` emits the versioned `mcpeval.probe-report/v1` document: the generator, server label, manifest SHA-256, per-case verdicts with tool names, fixed reason labels with their remediation hints, the declared bound behind a bound-based failure, measurement numbers, and a `readiness` object. There are no timestamps, sessions, or payloads, so the document is safe to commit as a baseline or attach to CI artifacts. `mcpeval schema report` prints its JSON Schema.

```json
{
  "schema": "mcpeval.probe-report/v1",
  "generator": {"name": "mcpeval", "version": "{{PRODUCT_VERSION}}"},
  "server": "demo",
  "manifest_sha256": "3f1c…",
  "passed": false,
  "readiness": {
    "score": 0,
    "categories": [{"name": "discovery", "passed": 0, "total": 1}]
  },
  "cases": [{
    "id": "catalog-budget",
    "probe": "discovery-cost",
    "tool": null,
    "passed": false,
    "attempts": 1,
    "first_failure": 1,
    "reason": "discovery-limit-exceeded",
    "hint": "…",
    "detail": {"bound": "max_tools", "limit": 10, "observed": 12},
    "measurements": {"tool_count": 12, "schema_bytes": 1802}
  }]
}
```

## Markdown

`--format markdown` renders a pull-request-ready report: verdict table, per-category breakdown, readiness score, and a static shields.io badge URL encoding only the score. No payload or server detail ever leaves the report.

## The readiness score

The score (0–100) is a deterministic composite over four weighted categories:

| Category | Weight | Probes |
| --- | --- | --- |
| discovery | 0.25 | `discovery-cost`, `token-cost`, `pagination`, `surface-listing` |
| reliability | 0.35 | `degradation-over-n`, `error-honesty`, `state-recovery`, `latency-budget`, `payload-bounds`, `cancellation`, `resource-subscription` |
| contract | 0.30 | `schema-guessability`, `instruction-fidelity`, `output-schema`, `protocol-negotiation`, `sampling`, `elicitation`, `completion` |
| concurrency | 0.10 | `contention` |

Each category contributes the fraction of its cases that passed, weighted as above. Categories with no cases in the manifest are excluded from both numerator and denominator, so a partial manifest is never penalized for probes it did not declare. The same report always produces the same score.

## Calibration

A score without a referent is just a number. mcp-eval ships a corpus of readiness observations from popular public MCP servers (`data/readiness-corpus.json`, refreshed by `scripts/corpus/collect.sh`). The corpus records its `battery`: the probe kinds every observation was scored on (`discovery-cost`, `token-cost`, `pagination`, `surface-listing` when the field is absent). Text and markdown reports score only the report's cases of that battery for the comparison, so a manifest with other cases is compared like for like, and count the observed servers that score is above, tied with, and below:

```text
  corpus battery (discovery-cost, token-cost, pagination, surface-listing): 100/100, above 1, tied with 33, below 0 of 34 observed servers
```

The readiness line still scores every case. A report with no case of the corpus battery prints no corpus line.

When observations carry `catalog_tokens`, a report with a token-cost measurement also places its catalog among them; the median is the lower middle for an even count:

```text
  catalog: 566 tokens over 12 tools, lighter than 20 of 34 observed servers (median 1915 tokens)
```

A personal or private corpus takes precedence when placed at `<MCPEVAL_HOME>/corpus.json`; when no corpus is available, reports omit both lines. JSON reports never carry corpus context. Calibration is deterministic: the same report against the same corpus always produces the same placement.

## Session cost

The token-cost probe's measurement is model-independent; interpreting it is the operator's call. Pass `--price-per-mtok <USD>` and the text and markdown reports translate the measurement into consequence: the catalog is charged to every session before the first tool call, so a 2,000-token catalog at $3/Mtok is $0.006 per session ($6 per 1,000 sessions) of pure context tax. Pricing never enters the JSON report — committed baselines stay byte-identical when prices change.

## SARIF and re-rendering

`--format sarif` emits a SARIF 2.1.0 document: one result per failing case, the probe kind as the rule id, the fixed reason plus remediation hint as the message, and the failing case's line in the manifest as the location. The manifest URI is relative to the working directory, so run the command from the repository root. Each server gets its own code-scanning category (`mcpeval/<server>/`). Upload it through GitHub code scanning and each failing case becomes an alert on its manifest line. `mcpeval report --format sarif` takes `--manifest` to locate results the same way. The document is deterministic and derived only from the sanitized report and the manifest's case lines.

Reports traveled as JSON stay useful offline: `mcpeval report <baseline.json> --format markdown` re-renders any committed `mcpeval.probe-report/v1` document without re-running a server, so the probe run and the report rendering can live in different jobs — or on different days. Re-rendering a failing document exits non-zero, so a rendered report can gate in its own right.

Two committed reports of the same server can also be compared directly: `mcpeval diff baseline.json current.json` classifies each case as regressed, fixed, changed (still failing, for a different reason), or unchanged and prints the readiness movement. With `--fail-on-regression` it exits non-zero for regressions, which makes the committed baseline a first-class CI gate; `--fail-on-change` also gates on changed failure reasons. See the continuous-integration guide for the recipe.

## Trends

Every full-battery run appends a content-free score record — server label, verdict counts, score, timestamp — to `<MCPEVAL_HOME>/store/probes/history.jsonl`. `mcpeval trends` renders the per-server history with score deltas between consecutive runs:

```sh
mcpeval trends --last 5
```

## Comparing servers

`mcpeval compare` runs one manifest against several targets and renders a side-by-side verdict and readiness grid in text, markdown, or JSON. Targets are `--endpoint LABEL=URL` Streamable HTTP endpoints, optionally plus one stdio command after `--`, whose column is labeled `stdio`; at least two targets are required. Comparison is informational and never gates; endpoint URLs follow the same loopback-first, credential-free policy as the probes.

```sh
mcpeval compare --server demo \
  --endpoint staging=http://127.0.0.1:8081/mcp \
  --endpoint candidate=http://127.0.0.1:8082/mcp

mcpeval compare --server demo \
  --endpoint staging=http://127.0.0.1:8081/mcp \
  -- ./target/release/mcpeval-demo
```
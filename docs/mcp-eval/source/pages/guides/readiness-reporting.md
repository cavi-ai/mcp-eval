# Readiness reporting

Every full-battery `mcpeval probe` run produces a deterministic, privacy-safe report in three formats.

## Text

The default format prints one line per case — verdict, attempts, first-failure position, fixed reason label, and measurement numbers — followed by a readiness summary line:

```text
literal-status instruction-fidelity pass attempts=1
demo readiness 87/100 (beats 40% of observed servers; corpus median 100) discovery=2/2 reliability=1/1 contract=1/1
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

`--format json` emits the versioned `mcpeval.probe-report/v1` document: server label, per-case verdicts, fixed reason labels, measurement numbers, and a `readiness` object. There are no timestamps, sessions, or payloads, so the document is safe to commit as a baseline or attach to CI artifacts.

```json
{
  "schema": "mcpeval.probe-report/v1",
  "server": "demo",
  "passed": true,
  "readiness": {
    "score": 100,
    "categories": [{"name": "discovery", "passed": 2, "total": 2}]
  }
}
```

## Markdown

`--format markdown` renders a pull-request-ready report: verdict table, per-category breakdown, readiness score, and a static shields.io badge URL encoding only the score. No payload or server detail ever leaves the report.

## The readiness score

The score (0–100) is a deterministic composite over four weighted categories:

| Category | Weight | Probes |
| --- | --- | --- |
| discovery | 0.25 | `discovery-cost`, `token-cost`, `pagination`, `surface-listing` |
| reliability | 0.35 | `degradation-over-n`, `error-honesty`, `state-recovery`, `latency-budget`, `payload-bounds` |
| contract | 0.30 | `schema-guessability`, `instruction-fidelity`, `output-schema` |
| concurrency | 0.10 | `contention` |

Each category contributes the fraction of its cases that passed, weighted as above. Categories with no cases in the manifest are excluded from both numerator and denominator, so a partial manifest is never penalized for probes it did not declare. The same report always produces the same score.

## Calibration

A score without a referent is just a number. mcp-eval ships a corpus of readiness observations from popular public MCP servers (`data/readiness-corpus.json`, refreshed by `scripts/corpus/collect.sh`), and every report places the score in that distribution:

```text
Readiness: 75/100 — beats 35% of observed servers (corpus median 100)
```

A personal or private corpus takes precedence when placed at `<MCPEVAL_HOME>/corpus.json`; when no corpus is available, reports simply omit the percentile line. Calibration is deterministic: the same score against the same corpus always produces the same percentile (midpoint method, so the median observation sits at 50).

## Trends

Every full-battery run appends a content-free score record — server label, verdict counts, score, timestamp — to `<MCPEVAL_HOME>/store/probes/history.jsonl`. `mcpeval trends` renders the per-server history with score deltas between consecutive runs:

```sh
mcpeval trends --last 5
```

## Comparing servers

`mcpeval compare` runs one manifest against several Streamable HTTP endpoints and renders a side-by-side verdict and readiness grid in text, markdown, or JSON. Comparison is informational and never gates; endpoint URLs follow the same loopback-first, credential-free policy as the probes.

```sh
mcpeval compare --server demo \
  --endpoint staging=http://127.0.0.1:8081/mcp \
  --endpoint candidate=http://127.0.0.1:8082/mcp
```
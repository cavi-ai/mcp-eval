# Readiness reporting

Every full-battery `mcpeval probe` run produces two results: the **gate**, which is your manifest's cases, and **readiness**, a 0-100 score under mcpeval's standard. The gate sets `passed` and the exit code. Readiness is measured by mcpeval's own read-only standard battery over the server's whole catalog, so two servers' scores mean the same thing whatever their manifests declare. `mcpeval score` runs the standard battery alone, with no manifest.

## Text

The default format prints one line per case — verdict, attempts, first-failure position, fixed reason label, and measurement numbers — then the gate, the readiness line with each area's score, the surface the standard saw, and every lost point. Each reason's hint prints once, under its first lost point:

```text
literal-status instruction-fidelity pass attempts=1
demo gate 1/1 passed
demo readiness 91/100 protocol=100 catalog=63 context=100 error-honesty=100 reliability=94 coverage=100 standard=mcpeval-standard/1
  surface: 12 tools, 10 read-only, 2 writers, 10 exercised
  lost catalog.description score=0 tool=describe_status observed=37 reason=catalog-short-description
    hint: describe what the tool does, when to use it, and what it returns in at least 40 characters; agents choose tools from this text
  lost catalog.output-schema score=0 tool=describe_status reason=catalog-no-output-schema
    hint: declare outputSchema and return structuredContent so agents can read results without parsing prose
  lost catalog.output-schema score=0 tool=read_counter reason=catalog-no-output-schema
  …
  lost reliability.consistent score=0 tool=flaky_read reason=reliability-inconsistent
    hint: the same read-only call with the same arguments ended differently across three calls; make read paths deterministic, or return a retryable error with a stable code
  lost reliability.latency score=50 tool=slow_read reason=reliability-slow
    hint: median latency is above 100 ms (score 80: up to 300 ms, 50: up to 1 s, 20: up to 3 s, 0: slower); cache or precompute the read, or page large results
  standard corpus (mcpeval-standard/1): above 26, tied 0, below 7 of 33 observed servers
  catalog: 566 tokens over 12 tools, lighter than 23 of 33 observed servers (median 1186 tokens)
```

Failing cases print a remediation hint: the concrete server-side fix for that fixed reason. A case that exceeded a manifest bound also names the bound, its limit, and the observed value:

```text
bounded-discovery discovery-cost fail attempts=1 first_failure=1 reason=discovery-limit-exceeded tools=12 schema_bytes=2262 bound=max_tools limit=10 observed=12
  hint: the catalog grew past the declared bounds; merge overlapping tools, drop tools agents never call, or negotiate a larger budget — a sprawling catalog taxes every session's context window
```

A `token-budget-exceeded` case also lists the three heaviest tools:

```text
tiny-budget token-cost fail attempts=1 first_failure=1 reason=token-budget-exceeded tools=12 total_tokens=566 bound=max_total_tokens limit=1 observed=566
  hint: the encoded catalog exceeds the token budget; …
  heaviest: report_weather 56, shared_read 56, elicited_read 49
```

Use `--brief` to suppress hints in scripts. The same hints appear in the markdown report under *Remediation*, and every reason is documented standalone:

```sh
mcpeval explain pagination-stalled-cursor
mcpeval explain        # list every fixed reason
```

## JSON

`--format json` emits the versioned `mcpeval.probe-report/v2` document: the generator, server label, manifest SHA-256, the `gate` counts, the `readiness` object, and per-case verdicts with tool names, fixed reason labels with their remediation hints, the declared bound behind a bound-based failure, and measurement numbers. `readiness` names its standard, the surface, and each area with its weight, score, and lost checks. There are no timestamps, sessions, arguments, or payloads, so the document is safe to commit as a baseline or attach to CI artifacts. `mcpeval schema report` prints its JSON Schema.

```json
{
  "schema": "mcpeval.probe-report/v2",
  "generator": {"name": "mcpeval", "version": "{{PRODUCT_VERSION}}"},
  "server": "demo",
  "manifest_sha256": "3f1c…",
  "passed": false,
  "gate": {"passed": 0, "total": 1},
  "readiness": {
    "standard": "mcpeval-standard/1",
    "score": 91,
    "badge": "https://img.shields.io/badge/mcpeval-91%2F100-brightgreen",
    "attested_read_only": false,
    "surface": {"tools": 12, "read_only": 10, "writers": 2, "exercised": 10},
    "areas": [
      {"name": "protocol", "weight": 15, "score": 100, "measurements": {}, "checks": []},
      {"name": "catalog", "weight": 20, "score": 63, "measurements": {},
       "checks": [{"id": "catalog.description", "tool": "describe_status", "score": 0,
                   "observed": 37, "reason": "catalog-short-description", "hint": "…"}, …]},
      {"name": "reliability", "weight": 20, "score": 94, "measurements": {},
       "checks": [{"id": "reliability.latency", "tool": "slow_read", "score": 50,
                   "observed": null, "reason": "reliability-slow", "hint": "…"}]}
    ]
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
    "measurements": {"tool_count": 12, "schema_bytes": 2262}
  }]
}
```

## Markdown

`--format markdown` renders a pull-request-ready report: the readiness score with its standard and a static shields.io badge URL encoding only the score, an area table, the surface, *Lost points* with their hints, the gate, and the verdict table. The verdict table's `Bound` column shows `<field> <observed> > <limit>` for a case that exceeded a manifest bound (`max_tools 12 > 10`), and *Remediation* lists the heaviest tools under a `token-budget-exceeded` case. No payload or server detail ever leaves the report.

## The readiness score

Readiness (0-100) is the weighted mean of the standard's areas, each scored on fixed curves that mcpeval defines. The weights, curves, bands, and checks are the standard; the report names it (`mcpeval-standard/1` in this build), and any change to them changes the name. No area ever drops out: surface the standard could not test counts against the score.

| Area | Weight | Scored as |
| --- | --- | --- |
| protocol | 15 | the share of these checks passed: an unknown method answers error -32601; `ping` answers an empty result; `tools/call` with an unknown tool name is refused; `tools/list` pages with finite cursors and valid, distinct entries; a declared resources or prompts capability lists cleanly (only when declared); `initialize` with an unknown version answers a date-shaped version the server supports and will echo |
| catalog | 20 | per tool, the share of its checks it passes, averaged over the catalog: a description of at least 40 characters; every input property described and typed (type, enum, const, `$ref`, anyOf, or oneOf), for tools with properties; `readOnlyHint` declared; `destructiveHint` declared, for writers; `outputSchema` declared. An empty catalog scores 0 |
| context | 15 | 75% the catalog's token estimate (100 at 2,000 tokens or fewer, 0 at 40,000 or more, logarithmic between) and 25% the heaviest tool's (100 at 500 or fewer, 0 at 5,000 or more) |
| reliability | 20 | each exercised tool's three calls: the same outcome every time, the median latency band (100 up to 100 ms, 80 up to 300 ms, 50 up to 1 s, 20 up to 3 s, else 0), and a declared `outputSchema` honored; plus one contention and one payload-bounds case on the first fully successful tool |
| error-honesty | 15 | the share of judged read-only tools that refuse one call with arguments violating their input schema — the required properties omitted, else the first typed property (by name) given the wrong type, else an unknown property when `additionalProperties` is `false` — with error -32602 or an `isError` result whose text says why, and still answer the next request. Tools whose schema admits every input are not judged; when none can be judged the area scores 0 (`honesty-untestable`) |
| coverage | 15 | exercised read-only tools over every read-only tool |

The standard battery is read-only by construction. It never calls a tool annotated `readOnlyHint: false` or `destructiveHint: true`, and it calls a tool with neither annotation only when you pass `--confirm-read-only`; otherwise that tool stays in the surface as unexercised (`coverage-unannotated`). Each tool's arguments are synthesized from its input schema, every required property by the first rule that applies: `$ref`; `const`, the first `enum` member, `default`, or the first of `examples`; the first `anyOf` or `oneOf` branch; then by type: a fixed sample for the `date`, `date-time`, `uri`, `email`, and `uuid` formats, else `mcpeval` padded or cut to `minLength` and `maxLength`; the minimum (or 1) for numbers; `false`; `minItems` copies of the item; nested objects the same way. A required string with only a `pattern`, or a property with no rule, leaves the tool unexercised (`coverage-unsynthesizable`). The battery trusts `readOnlyHint`: a server that marks a writing tool read-only gets it called. `--skip-tool <NAME>` (repeatable) never calls that tool; a skipped tool the battery would have called scores 0 for reliability (`reliability.skipped`) and error honesty, and any skip scores the contention and payload cases 0, so skipping never raises the score. `--gate-only` skips the battery entirely. Each call waits at most 10 seconds; a call that times out or ends the connection costs that tool only, and the next tool starts on a fresh connection. The battery runs on its own connection after the gate and never writes to the call journal. Before a read-only tool's ordinary calls, the battery makes one call with schema-violating arguments for the error-honesty area. The protocol checks run last: they send an unknown method, `ping`, a `tools/call` naming a tool that does not exist, the paged listings, and extra `initialize` handshakes. A page after the first that fails ends the catalog with the tools already listed and costs `protocol.pagination`.

It runs on full-battery `probe` runs and on `compare`; `--gate-only` skips it, and `--probe <kind>` and `verify` never run it. `mcpeval explain` covers the gate's reasons; each lost readiness check carries its own hint.

## Calibration

The corpus (`data/readiness-corpus.json`, `mcpeval.readiness-corpus/v2`, refreshed by `scripts/corpus/collect.sh`) holds the readiness and area scores of popular public servers under one named standard. Text and markdown reports from `probe` and `score` place your readiness among them when your report was scored under the same standard, counting ties explicitly, and place your catalog among the servers that recorded `catalog_tokens`; the median is the lower middle for an even count:

```text
  standard corpus (mcpeval-standard/1): above 26, tied 0, below 7 of 33 observed servers
  catalog: 566 tokens over 12 tools, lighter than 23 of 33 observed servers (median 1186 tokens)
```

A personal or private corpus takes precedence when placed at `<MCPEVAL_HOME>/corpus.json`; a v1 corpus (manifest pass rates) is not read. When no corpus is available, reports omit both lines. JSON reports never carry corpus context.

## Session cost

The token-cost probe's measurement is model-independent; interpreting it is the operator's call. Pass `--price-per-mtok <USD>` and the text and markdown reports translate the measurement into consequence: the catalog is charged to every session before the first tool call, so a 2,000-token catalog at $3/Mtok is $0.006 per session ($6 per 1,000 sessions) of pure context tax. Pricing never enters the JSON report — committed baselines stay byte-identical when prices change.

## SARIF and re-rendering

`--format sarif` emits a SARIF 2.1.0 document: one result per failing case, the probe kind as the rule id, the fixed reason plus remediation hint as the message, and the failing case's line in the manifest as the location. The manifest URI is relative to the working directory, so run the command from the repository root. Each server gets its own code-scanning category (`mcpeval/<server>/`). Upload it through GitHub code scanning and each failing case becomes an alert on its manifest line. `mcpeval report --format sarif` takes `--manifest` to locate results the same way. The document is deterministic and derived only from the sanitized report and the manifest's case lines.

Reports traveled as JSON stay useful offline: `mcpeval report <baseline.json> --format markdown` re-renders any committed `mcpeval.probe-report/v1` or `/v2` document without re-running a server, so the probe run and the report rendering can live in different jobs — or on different days. Re-rendering a failing document exits non-zero, so a rendered report can gate in its own right.

Two committed reports of the same server can also be compared directly: `mcpeval diff baseline.json current.json` classifies each case as regressed, fixed, changed (still failing, for a different reason), or unchanged and prints the readiness movement. Readiness moves only when both documents were scored under the same standard; against a v1 baseline or a different standard the diff says `not comparable` and still classifies every case. With `--fail-on-regression` it exits non-zero for regressions, which makes the committed baseline a first-class CI gate; `--fail-on-change` also gates on changed failure reasons. See the continuous-integration guide for the recipe.

## Trends

Every full-battery run that measured readiness appends a content-free record — server label, gate counts, readiness score, its standard, manifest SHA-256, timestamp — to `<MCPEVAL_HOME>/store/probes/history.jsonl`; `--gate-only` runs record none. `mcpeval trends` renders the per-server history with a score delta between consecutive runs under the same standard, `standard changed` where the standard differs, and the first eight hex digits of each run's manifest hash:

```sh
mcpeval trends --last 5
```

```text
demo
  2026-09-30T18:03:04.761Z score=91/100 cases=1/1 manifest=0524c76b
  2026-09-30T18:03:50.618Z score=90/100 cases=1/1 -1 manifest=0524c76b
```

Records written before the standard existed hold a manifest pass rate: they compare only with each other, by manifest hash.

## Comparing servers

`mcpeval compare` runs one manifest against several targets and renders a side-by-side verdict and readiness grid in text, markdown, or JSON; `--gate-only` compares the manifest verdicts alone. Targets are `--endpoint LABEL=URL` Streamable HTTP endpoints, optionally plus one stdio command after `--`, whose column is labeled `stdio`; at least two targets are required. Comparison is informational and never gates; endpoint URLs follow the same loopback-first, credential-free policy as the probes.

```sh
mcpeval compare --server demo \
  --endpoint staging=http://127.0.0.1:8081/mcp \
  --endpoint candidate=http://127.0.0.1:8082/mcp

mcpeval compare --server demo \
  --endpoint staging=http://127.0.0.1:8081/mcp \
  -- mcpeval-demo
```
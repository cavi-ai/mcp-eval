# State of MCP servers

How healthy are the MCP servers that agents actually use? This page is produced by scoring popular public servers with mcpeval's read-only standard battery (`mcpeval score`, readiness standard `mcpeval-standard/1`) and publishing the raw distribution. No self-reported scores, no vendor claim: every number here comes from the battery and is reproducible by anyone with the CLI.

These are historical observations under `mcpeval-standard/1`. Current scoring uses `mcpeval-standard/2`, which corrects success credit and output-schema validation. These scores are retained under their original standard and are not used for current readiness comparisons.

## The corpus

{{PRODUCT_VERSION}} ships a corpus of **33 public MCP servers** collected across the npm and uvx ecosystems (the reference servers plus the most-downloaded community servers). Each server ran as installed, without credentials, with no Kubernetes context and no Docker daemon.

| Observation | Count |
| --- | --- |
| Readiness 100/100 | 0 |
| Readiness below 100 | 33 |
| Median readiness | 42 |
| Servers whose tools the battery may not call (no `readOnlyHint: true`) | 20 |

| Server | Readiness | Protocol | Catalog | Context | Error honesty | Reliability | Coverage | Tools | Catalog tokens |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `sequential-thinking` | 99 | 100 | 100 | 91 | 100 | 100 | 100 | 1 | 1160 |
| `memory` | 97 | 100 | 89 | 93 | 100 | 100 | 100 | 9 | 2688 |
| `wikipedia-uvx` | 96 | 100 | 83 | 100 | 100 | 98 | 100 | 7 | 1021 |
| `filesystem` | 95 | 100 | 84 | 88 | 100 | 100 | 100 | 14 | 3246 |
| `searxng` | 95 | 100 | 80 | 93 | 100 | 100 | 100 | 4 | 1745 |
| `context7` | 93 | 100 | 80 | 95 | 100 | 91 | 100 | 2 | 1216 |
| `everything` | 93 | 100 | 72 | 100 | 100 | 95 | 100 | 13 | 1915 |
| `arxiv` | 89 | 83 | 76 | 80 | 100 | 97 | 100 | 19 | 4463 |
| `time` | 89 | 80 | 60 | 100 | 100 | 100 | 100 | 2 | 307 |
| `git` | 87 | 80 | 52 | 100 | 100 | 100 | 100 | 12 | 1479 |
| `desktop-commander` | 77 | 100 | 63 | 36 | 64 | 99 | 100 | 26 | 14361 |
| `kubernetes` | 73 | 100 | 62 | 65 | 33 | 100 | 70 | 23 | 5965 |
| `notion` | 65 | 100 | 64 | 32 | 0 | 89 | 100 | 24 | 19057 |
| `airbnb` | 42 | 100 | 60 | 100 | 0 | 0 | 0 | 2 | 538 |
| `docker` | 42 | 100 | 60 | 100 | 0 | 0 | 0 | 1 | 93 |
| `markitdown` | 42 | 100 | 60 | 100 | 0 | 0 | 0 | 1 | 105 |
| `terraform` | 42 | 100 | 60 | 100 | 0 | 0 | 0 | 10 | 1618 |
| `browserbase` | 41 | 100 | 54 | 100 | 0 | 0 | 0 | 9 | 1334 |
| `tavily` | 41 | 100 | 60 | 96 | 0 | 0 | 0 | 5 | 1923 |
| `pandoc` | 40 | 100 | 60 | 88 | 0 | 0 | 0 | 1 | 1507 |
| `puppeteer` | 40 | 100 | 49 | 100 | 0 | 0 | 0 | 7 | 614 |
| `calculator` | 39 | 80 | 60 | 100 | 0 | 0 | 0 | 1 | 123 |
| `docker-mcp` | 39 | 100 | 46 | 100 | 0 | 0 | 0 | 10 | 906 |
| `fetch` | 39 | 83 | 60 | 100 | 0 | 0 | 0 | 1 | 276 |
| `sqlite` | 39 | 100 | 46 | 100 | 0 | 0 | 0 | 10 | 1186 |
| `sqlite-npx` | 38 | 100 | 38 | 100 | 0 | 0 | 0 | 8 | 775 |
| `wikipedia-npm` | 38 | 100 | 40 | 100 | 0 | 0 | 0 | 2 | 141 |
| `github` | 37 | 100 | 48 | 83 | 0 | 0 | 0 | 26 | 3967 |
| `playwright` | 37 | 100 | 47 | 86 | 0 | 0 | 0 | 33 | 3467 |
| `todoist` | 36 | 100 | 28 | 100 | 0 | 0 | 0 | 5 | 408 |
| `mermaid` | 35 | 100 | 23 | 100 | 0 | 0 | 0 | 4 | 411 |
| `ollama` | 35 | 100 | 24 | 100 | 0 | 0 | 0 | 9 | 775 |
| `postgres` | 31 | 83 | 20 | 100 | 0 | 0 | 0 | 1 | 33 |

## What the data says

**Missing annotations cost the most.** 20 of 33 servers mark no tool `readOnlyHint: true`, so the standard battery calls none of their tools: error honesty, reliability, and coverage score 0, and these servers land between 31 and 42. Across the corpus, 173 of 302 tools declare no `readOnlyHint` at all. The annotation is one line per tool, and it is what lets any client tell a read from a write.

**Where tools can be called, most servers behave.** The 13 servers with callable tools score 65 to 99. Ten of them refuse every schema-violating call with -32602 or an `isError` result that says why. `notion` accepted invalid arguments on 11 tools, `desktop-commander` on 5, and `kubernetes` refused 4 with a JSON-RPC code other than -32602.

**Catalogs rarely describe their results.** 269 of 302 tools declare no `outputSchema`, and 27 of 33 servers declare none on any tool; 96 tools leave input parameters undescribed and 70 have descriptions under 40 characters. Only `sequential-thinking` scores 100 on the catalog area; the median is 60.

**The catalog tax is universal — and measurable.** Every session pays the full `tools/list` catalog before the first tool call (deterministic estimate: encoded bytes / 4): the reference `everything` server costs 1,915 tokens with 13 tools, `notion` 19,057 with 24 tools, `desktop-commander` 14,361 with 26 tools, while `markitdown` costs 105 with one tool. The corpus median is 1,186 catalog tokens. At $3/Mtok, `notion`'s catalog is $0.057 per session — $57 per 1,000 sessions before any useful work happens.

**Protocol basics mostly hold.** 27 of 33 servers score 100 on the protocol area. `arxiv`, `fetch`, `git`, and `time` answer an unknown JSON-RPC method with a result instead of error -32601; `calculator` echoes a protocol version it does not support; `postgres` declares a surface whose listing does not answer.

**The 2025-06-18 interactive surface is barely adopted.** A capability sweep over the corpus found `sampling` on zero servers, `elicitation` on zero servers, and `resources.subscribe` on two (the reference `everything` server and `memory`). The reference server's `instructions` string references `trigger-sampling-request` and `trigger-elicitation-request`, but those tools only enter the catalog when the client declares the matching capability (13 tools for a capability-less client, 16 with sampling and elicitation declared). Instructions that promise tools an agent cannot see in its own catalog are a coherence defect; the finding was filed upstream as [modelcontextprotocol/servers#4792](https://github.com/modelcontextprotocol/servers/issues/4792).

## Collect a current corpus

Collect new observations under the current evaluator standard. The historical scores above require the earlier evaluator and corresponding server artifacts:

```sh
cargo build --release
scripts/corpus/collect.sh          # rebuilds data/readiness-corpus.json
mcpeval score --server your-server -- your-mcp-server --flags
```

Text and markdown reports place a readiness score among these servers when it was scored under the same standard. This is a historical standard/1 placement example; standard/2 reports do not place readiness against this snapshot:

```text
  standard corpus (mcpeval-standard/1): above 26, tied 0, below 7 of 33 observed servers
```

## Method notes

- The standard battery is read-only: it calls only tools annotated `readOnlyHint: true` (never `readOnlyHint: false` or `destructiveHint: true`), with arguments synthesized from each tool's input schema, plus one schema-violating call per judged tool, protocol requests, and the paged listings. Unannotated tools count as unexercised. The readiness-reporting guide lists every area and check.
- Servers that need credentials or backing services ran without them, so their tools may answer with errors; `mcp-atlassian` lists no tools without credentials and is not in the corpus.
- Every server runs with an empty kubeconfig and an unreachable Docker host, so no call reaches a real cluster or container daemon.
- Re-runs on one machine moved only reliability, by at most 3 points: latency bands depend on the machine and network, and they move more between machines. Some servers also describe their tools per operating system — `desktop-commander`'s descriptions name the platform and its shell — so the corpus records the platform it was collected on (macOS) and is re-scored there.
- This snapshot lacks package-version, evaluator, and report-artifact provenance. Its original execution inputs cannot be recovered from these observations, so the current drift checker refuses to replay it using floating packages.
- New v3 collections require an explicit pinned target file and declared credential/service checks. They retain every target outcome and distinguish untested targets from completed calls that failed. Verification requires the original platform, evaluator, target file, and report artifacts; unrun targets prevent a pass. Score differences do not establish their cause: transitive dependencies and backing-service state still need a separately locked environment.

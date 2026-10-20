# State of MCP servers

This page reports popular public MCP servers scored with mcpeval's read-only
standard battery (`mcpeval score`) under readiness standard
`mcpeval-standard/2`. The corpus behind it, `data/readiness-corpus.json`, was
collected with MCP Eval 0.7.1 on macOS from the pinned target file `data/corpus-targets.json` and
records the evaluator, the target file's SHA-256, and each target's package,
version, and outcome.

## The corpus

The target file pins 34 servers from the npm and uvx ecosystems: the
reference servers plus widely used community servers. Each server ran as
installed, without credentials, with no Kubernetes context and no Docker
daemon. A target is observed only when the battery called at least one of its
tools; the rest stay in the corpus population with the reason. The corpus
holds **13 public MCP servers** observed under the standard.

| Outcome | Targets |
| --- | --- |
| Observed | 13 |
| Untested: no tool the battery may call | 19 |
| Untested: no tools listed without credentials (`mcp-atlassian`) | 1 |
| Errored: the evaluation did not finish (`docker-mcp`) | 1 |

| Observation | Count |
| --- | --- |
| Readiness 100/100 | 0 |
| Readiness below 100 | 13 |
| Median readiness | 69 |

| Server | Readiness | Protocol | Catalog | Context | Error honesty | Reliability | Coverage | Tools | Catalog tokens |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `memory` | 97 | 100 | 89 | 92 | 100 | 100 | 100 | 9 | 2699 |
| `wikipedia-uvx` | 97 | 100 | 85 | 100 | 100 | 98 | 100 | 8 | 1142 |
| `fetch` | 94 | 100 | 80 | 100 | 100 | 92 | 100 | 1 | 287 |
| `everything` | 93 | 100 | 74 | 94 | 100 | 97 | 100 | 16 | 2536 |
| `context7` | 81 | 100 | 80 | 95 | 100 | 65 | 50 | 2 | 1305 |
| `searxng` | 81 | 100 | 80 | 93 | 100 | 67 | 50 | 4 | 1745 |
| `filesystem` | 69 | 100 | 84 | 89 | 100 | 27 | 20 | 14 | 3139 |
| `arxiv` | 66 | 83 | 76 | 80 | 100 | 35 | 27 | 19 | 4463 |
| `notion` | 66 | 100 | 64 | 32 | 0 | 91 | 100 | 24 | 19057 |
| `desktop-commander` | 65 | 100 | 64 | 39 | 62 | 66 | 62 | 25 | 13341 |
| `time` | 61 | 100 | 80 | 100 | 100 | 0 | 0 | 2 | 575 |
| `git` | 56 | 100 | 54 | 100 | 100 | 0 | 0 | 12 | 1515 |
| `kubernetes` | 52 | 100 | 62 | 65 | 33 | 33 | 20 | 23 | 5965 |

Observed readiness ranges from 52 to 97, with a median of 69; no server scores
100.

## What the data says

**Missing annotations decide whether a server can be measured.** 18 servers
declare `readOnlyHint` on none of their tools, so the battery calls nothing and
they are untested: `airbnb`, `browserbase`, `calculator`, `docker`, `github`,
`markitdown`, `mermaid`, `ollama`, `pandoc`, `playwright`, `postgres`,
`puppeteer`, `sqlite`, `sqlite-npx`, `tavily`, `terraform`, `todoist`, and
`wikipedia-npm`. `sequential-thinking` 2.0.0 annotates its only tool as a
writer. Across the 33 servers that answered, 162 of 295 tools declare no
`readOnlyHint`. The annotation is one line per tool, and it is what lets any
client tell a read from a write.

**Calls with values built from the schema often fail.** Reliability and
coverage fall where a tool needs a real path, ID, or service: every call of
`time` and `git` failed, as did most calls of `filesystem` and `arxiv`. Pass
working values with `--arguments` to measure those tools; see
[Readiness reporting](readiness-reporting.md).

**Most servers refuse invalid arguments honestly.** Ten of the 13 observed
servers refuse every schema-violating call with -32602 or an `isError` result
that says why. `notion` accepted invalid arguments on 11 tools,
`desktop-commander` on 5, and `kubernetes` refused 4 with a JSON-RPC code
other than -32602.

**Catalogs rarely describe their results.** 258 of 295 tools declare no
`outputSchema`, and 25 of the 32 servers that list tools declare none on any
tool. 95 tools leave input parameters undescribed and 64 have descriptions
under 40 characters. No observed server scores 100 on the catalog area; the
observed median is 80.

**The catalog tax is measurable.** Every session pays the full `tools/list`
catalog before the first tool call (deterministic estimate: encoded bytes / 4):
the reference `everything` server costs 2,536 tokens with 16 tools, `notion`
19,057 with 24 tools, and `desktop-commander` 13,341 with 25 tools, while
`markitdown` costs 105 with one tool. The corpus median is 2,536 catalog
tokens. At $3/Mtok, `notion`'s catalog is $0.057 per session,
$57 per 1,000 sessions, before any useful work happens.

**Protocol basics mostly hold.** 29 of the 33 servers that answered score 100
on the protocol area. `arxiv` and `mcp-atlassian` answer an unknown JSON-RPC
method with a result instead of error -32601; `calculator` echoes a protocol
version it does not support; `postgres` declares a surface whose listing does
not answer.

## Collect the corpus

The corpus is collected from a reviewed repository checkout with the release
binary built at that checkout:

```sh
cargo build --release --locked --bins
scripts/corpus/collect.sh --targets data/corpus-targets.json \
  --out /absolute/private/corpus.json
```

A new readiness standard recollects the corpus: the repository's tests require
`data/readiness-corpus.json` to name the standard the build scores under and
the SHA-256 of `data/corpus-targets.json`. Follow
[Corpus collection and replay](corpus-replay.md) for prerequisites, minimum
observation counts, saved evidence, offline checks, and CI consumption.

Text and markdown reports place a readiness score among the observed servers
when it was scored under the same standard and measurement profile:

```text
  standard corpus (mcpeval-standard/2): above 9, tied 0, below 4 of 13 observed servers
```

## Method notes

- The standard battery is read-only: it calls only tools annotated
  `readOnlyHint: true` (never `readOnlyHint: false` or `destructiveHint: true`),
  with arguments synthesized from each tool's input schema, plus one
  schema-violating call per judged tool, protocol requests, and the paged
  listings. Unannotated tools count as unexercised. The readiness-reporting
  guide lists every area and check.
- Servers that need credentials or backing services ran without them, so their
  tools may answer with errors.
- Every server runs with an empty kubeconfig, an unreachable Docker host, and
  its own empty home directory, so no call reaches a real cluster, container
  daemon, or user configuration.
- Latency bands depend on the machine and network, and some servers describe
  their tools per operating system, so the corpus records the platform it was
  collected on.
- Exact package pins do not lock transitive dependencies or backing-service
  state. Score differences between collections do not establish their cause.

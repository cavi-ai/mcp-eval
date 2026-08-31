# State of MCP servers

How healthy are the MCP servers that agents actually use? This page is produced by running the mcp-eval deterministic battery — the same probes described in the evaluation-dimensions reference — against popular public servers, and publishing the raw distribution. No self-reported scores, no vendor claim: every number here is a deterministic verdict from the battery, reproducible by anyone with the CLI.

## The corpus

{{PRODUCT_VERSION}} ships a corpus of **16 public MCP servers** collected across the npm and uvx ecosystems (the reference servers plus the most-downloaded community servers that run without live credentials). Each server was probed with the same generic battery: discovery bounds, token budget, cursor pagination, and declared-surface listing.

| Observation | Count |
| --- | --- |
| Readiness 100/100 | 15 |
| Readiness below 100 | 1 |
| Servers that could not complete the battery unaided | many require live credentials or services and are excluded |

## What the data says

**The catalog tax is universal.** Even at readiness 100, every session pays the full `tools/list` catalog before the first tool call. The probed servers range from a handful of tools to dozens; a 40-tool catalog at typical descriptions costs roughly 2,000 tokens per session — about $0.006 per session at $3/Mtok, $6 per 1,000 sessions, before any useful work happens. `token-cost` is the headline number for anyone running agents at scale.

**Perfection is the norm for active servers — which makes the exceptions information-rich.** 15 of 16 servers score 100/100: maintainers who ship coherent schemas, stable error codes, and bounded pagination are already meeting the contract this battery verifies. The interesting signal is the rest: the most common defect in the corpus is a **declared surface that does not answer** — a server advertising `resources` (or `prompts`) whose listing errors or returns a malformed envelope. That is exactly what the `surface-listing` probe exists to catch, and it is invisible to every client that never asks.

**Cursor pagination is where trust breaks.** Servers that paginate tool catalogs must do so without repeating entries and with terminating cursors. The battery treats a re-served page and an unending cursor as distinct, named defects; both were found in the wild while developing the probe.

## Reproduce it

Every number on this page can be regenerated:

```sh
cargo build --release
scripts/corpus/collect.sh          # rebuilds data/readiness-corpus.json
mcpeval probe --server your-server --format markdown -- your-mcp-server --flags
```

Your own score is placed into the same distribution automatically — the corpus ships with the binary, and the readiness line in every report reads *beats N% of observed servers*. Add your server to the corpus by opening a PR with a refreshed `data/readiness-corpus.json` produced by the script above; no special access is required.

## Method notes

- The battery is purely structural and read-only: catalog shape, schema coherence, pagination, cursor termination, and declared-surface listings. It never calls mutating tools and never inspects payload content.
- Scores are deterministic — the same server and the same battery produce the same verdict. Corpus refreshes happen at release time; a server that fixes its defects moves up when its observation is refreshed.
- Servers requiring live credentials or backing services are probed with real credentials by the maintainers where possible and excluded where not; the corpus only claims what the battery actually ran against.
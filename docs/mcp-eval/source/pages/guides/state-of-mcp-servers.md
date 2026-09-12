# State of MCP servers

How healthy are the MCP servers that agents actually use? This page is produced by running the mcp-eval deterministic battery — the same probes described in the evaluation-dimensions reference — against popular public servers, and publishing the raw distribution. No self-reported scores, no vendor claim: every number here is a deterministic verdict from the battery, reproducible by anyone with the CLI.

## The corpus

{{PRODUCT_VERSION}} ships a corpus of **34 public MCP servers** collected across the npm and uvx ecosystems (the reference servers plus the most-downloaded community servers that run without live credentials). Each server was probed with the same generic battery: discovery bounds, token budget, cursor pagination, and declared-surface listing.

| Observation | Count |
| --- | --- |
| Readiness 100/100 | 33 |
| Readiness below 100 | 1 |
| Servers that could not complete the battery unaided | many require live credentials or services and are excluded |

## What the data says

**The catalog tax is universal — and measurable.** Every session pays the full `tools/list` catalog before the first tool call. Measured token budgets across the corpus (deterministic estimate: encoded bytes / 4): the reference `everything` server costs 1,915 tokens per session with 13 tools, `notion` costs 19,057 with 24 tools, `desktop-commander` 15,320 with 26 tools, while `markitdown` costs 68 with one tool. At $3/Mtok, `notion`'s catalog is $0.057 per session — $57 per 1,000 sessions of pure context tax before any useful work happens. `token-cost` is the headline number for anyone running agents at scale.

**Perfection is the norm for active servers — which makes the exceptions information-rich.** 33 of 34 servers score 100/100: maintainers who ship coherent schemas, stable error codes, and bounded pagination are already meeting the contract this battery verifies. The single sub-100 score (`postgres`, 75) is the declared-surface defect class: the server advertises surfaces whose listing does not answer — exactly what the `surface-listing` probe exists to catch, and it is invisible to every client that never asks.

**Cursor pagination is where trust breaks.** Servers that paginate tool catalogs must do so without repeating entries and with terminating cursors. The battery treats a re-served page and an unending cursor as distinct, named defects; both were found in the wild while developing the probe.

**The 2025-06-18 interactive surface is barely adopted.** A capability sweep over the corpus found `sampling` on zero servers, `elicitation` on zero servers, and `resources.subscribe` on two (the reference `everything` server and `memory`). The new `protocol-negotiation`, `sampling`, `elicitation`, and `resource-subscription` probes therefore pass trivially on nearly all of the corpus — the interesting finding is what is *conditionally declared*: the reference server's `instructions` string unconditionally references `trigger-sampling-request` and `trigger-elicitation-request`, but those tools only enter the catalog when the client declares the matching capability (13 tools for a capability-less client, 16 with sampling and elicitation declared). Instructions that promise tools an agent cannot see in its own catalog are exactly the coherence class the battery is built to expose; the finding was filed upstream as [modelcontextprotocol/servers#4792](https://github.com/modelcontextprotocol/servers/issues/4792).

## Reproduce it

Every number on this page can be regenerated:

```sh
cargo build --release
scripts/corpus/collect.sh          # rebuilds data/readiness-corpus.json
mcpeval probe --server your-server --format markdown -- your-mcp-server --flags
```

Your own score is placed into the same distribution automatically — the corpus ships with the binary, and the readiness line in every report reads *beats N% of observed servers*. Add your server to the corpus by opening a PR with a refreshed `data/readiness-corpus.json` produced by the script above; no special access is required.

## Method notes

- The battery is purely structural and read-only: catalog shape, schema coherence, pagination, cursor termination, declared-surface listings, and protocol-version selection. It never calls mutating tools and never inspects payload content. The interactive-surface probes (`sampling`, `elicitation`) declare the corresponding client capability so servers may exercise their sub-request flow, but they never provide model inference or user input beyond deterministic stubs.
- Scores are deterministic — the same server and the same battery produce the same verdict. Corpus refreshes happen at release time; a server that fixes its defects moves up when its observation is refreshed.
- Reproducibility is enforced, not assumed: a scheduled workflow re-probes every corpus observation with the current binary and fails on any score that moved (`node scripts/corpus/verify.mjs` locally). A moved score means the server changed — refreshes are deliberate and explained in the pull request that commits them.
- Servers requiring live credentials or backing services are probed with real credentials by the maintainers where possible and excluded where not; the corpus only claims what the battery actually ran against.
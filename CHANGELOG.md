# Changelog

All notable changes to this project are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and the project uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Stdio MCP shim with privacy-preserving call capture.
- JSONL store, SQLite index, annotations, issue promotion, and findings reports.
- Deterministic `degradation-over-n` and `instruction-fidelity` probes.
- Strict `mcp-eval.manifest.json` validation and explicit sandbox authorization
  for mutating probes.
- Clean and broken fixture-server integration coverage.
- Stable privacy-safe finding IDs and persistent finding lifecycle state.
- Targeted `verify` runs with three-consecutive-green closure, automatic
  regression reopening, and append-only probe history.
- Deterministic `discovery-cost` and `schema-guessability` probes with bounded,
  privacy-safe measurements.
- Deterministic `error-honesty` and `state-recovery` probes with bounded retries,
  fixed failure reasons, and clean/broken fixture coverage.
- Two-client `contention` probes with synchronized execution and sanitized records
  for both calls.
- Streamable HTTP probe transport with JSON and SSE responses, session propagation,
  bounded I/O, and loopback-first endpoint policy.
- Loopback-only Streamable HTTP capture proxy with bounded forwarding and the existing
  privacy-safe call-record boundary.
- Read-only, sandbox-free manifest generation for eligible promoted findings, with
  explicit operator attestation and output overwrite authorization.
- Deterministic `token-cost` probe with total and per-tool token budgets from a
  model-independent estimator over each tool's `tools/list` entry.
- `mcpeval probe --format json`: versioned, deterministic
  `mcpeval.probe-report/v1` report for CI artifacts and committed baselines.
- `mcpeval init`: scaffolds a strict starter manifest from a live server's
  `tools/list` catalog, deriving discovery/token budgets from measured sizes
  and smoke-testing each candidate tool with a naive read-only call before
  declaring it; refuses to overwrite an existing manifest without `--force`.
- `mcpeval schema`: prints the embedded draft-2020-12 JSON Schema for
  `mcp-eval.manifest.json` for editor validation.
- Readiness score (0-100): deterministic, privacy-safe composite over four
  weighted categories, included additively in `mcpeval.probe-report/v1` and
  rendered by the text and markdown report formats. Partial manifests score
  only on the categories they declare.
- `mcpeval probe --format markdown`: pull-request-ready report with a
  per-category breakdown, readiness score, and a static shields.io badge URL.
- `latency-budget` probe: read-only calls against a declared millisecond
  budget with the slowest observed latency reported and a fixed
  `latency-budget-exceeded` failure reason.
- `pagination` probe: cursor-driven `tools/list` traversal with entry
  validation, duplicate-tool detection across pages, and a stalled-cursor
  bound; servers that do not paginate pass trivially.
- `mcpeval compare`: run one manifest against several Streamable HTTP
  endpoints and render a side-by-side verdict and readiness grid in text,
  markdown, or JSON; informational by design and never gates.
- `mcpeval export-issues`: one GitHub-issue-ready markdown file per promoted
  finding with severity labels, evidence, shape-level repro, and the
  `generate`/`verify` next steps.
- `mcpeval trends` and readiness-score history: full-battery probe runs append
  content-free score records to `<MCPEVAL_HOME>/store/probes/history.jsonl`.
- `mcpeval serve`: loopback-only Streamable HTTP MCP server exposing
  `list_findings`, `get_finding`, and `get_readiness_trends` so coding agents
  can query their own findings natively; serves only share-safe content.
- Composite GitHub Action (`action.yml`) and a CI gating guide
  ([docs/ci.md](docs/ci.md)) covering baseline diffs and readiness badges.
- `mcpeval-demo`, a bundled MCP stdio server with a clean personality and
  `--broken <aspect>` personalities (`schema`, `fidelity`, `unstable-errors`,
  `bloated`, `duplicate-page`, `stalled-cursor`, `slow`) that reproduce each
  probe's fixed failure reason for zero-infrastructure onboarding.
- `payload-bounds` probe: injects one exact-size argument to verify the server
  never crashes or hangs under load; a clean structured rejection is honest
  bounded behavior unless `expect_handled` asserts the size must be handled.
- `surface-listing` probe: envelope-shape and cursor-bound checks over declared
  `resources`/`prompts` surfaces; undeclared surfaces pass trivially.
- `output-schema` probe: tools declaring `outputSchema` must return
  `structuredContent` covering the schema's required fields.
- Demo aspects `surface` and `output-schema` exercising the new probes, and a
  `report_weather` tool with structured output.
- Readiness categories rebalanced for the deeper battery: discovery 0.25,
  reliability 0.35, contract 0.30, concurrency 0.10.
- Remediation hints: every fixed failure reason maps to a concrete server-side
  fix, printed under failing cases in text output (suppress with `--brief`),
  rendered in the markdown report under *Remediation*, and documented
  standalone via the new `mcpeval explain <reason>` command.
- Calibrated readiness scores: a checked-in corpus of observations from
  popular public MCP servers (`data/readiness-corpus.json`, refreshed by
  `scripts/corpus/collect.sh`) places every score in the observed
  distribution — percentile and corpus median appear in text and markdown
  reports; a private override at `<MCPEVAL_HOME>/corpus.json` wins.
- Agent-loop tools on the `mcpeval serve` surface: `run_probe` executes the
  deterministic battery with an inline manifest against any targeted server
  and returns the full versioned report plus remediation hints (mutation is
  never authorized through this surface), and `scaffold` derives a starter
  manifest from a live catalog in memory. The scaffold → probe → fix →
  re-run loop now runs natively inside the agent's own MCP protocol.
- Stdio client interoperability hardening from dogfooding against the official
  reference server: stdout banners and unsolicited notifications no longer
  abort a session, while mismatched response ids still fail fast; a server
  that never answers times out instead of echoing prose.

### Security

- Probe calls remain read-only by default and use the existing sanitized
  persistence boundary.
- Unprobeable findings are capped at medium severity, and verification validates
  the finding/case/tool relationship before launching a server process.

# Changelog

All notable changes to this project are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and the project uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] - 2026-08-30

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
- `cancellation` probe: issues a read-only call, cancels it with
  `notifications/cancelled`, and requires silence for the cancelled request
  id within a declared grace window; preflight confirms the tool succeeds
  uncancelled. Stdio targets; Streamable HTTP fails closed with a fixed
  reason because its synchronous response cannot observe a mid-flight
  cancellation. The demo server gained a `--broken cancellation` aspect that
  drops the notification, and its slow tool now honors cancellation through
  a dedicated stdin reader thread.
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
- Session-cost interpretation: `probe --price-per-mtok <USD>` translates the
  measured catalog tokens into cost per session and per 1,000 sessions in
  text and markdown reports; the JSON report stays price-free so committed
  baselines remain deterministic.
- `mcpeval share --dir <DIR>`: packages the share-safe envelope
  mechanically — store records plus a SHARE.md manifest of contents and
  exclusions, produced only after a clean redaction sweep, with loud
  warnings when annotation notes need review. The salt, index databases,
  and manifests are never copied.
- `probe --format sarif`: SARIF 2.1.0 output for GitHub code scanning; one
  result per failing case with the fixed reason and remediation hint as the
  message, deterministic and derived only from the sanitized report.
- `mcpeval report <document>`: re-render a committed
  `mcpeval.probe-report/v1` document (baseline or CI artifact) as text,
  markdown, or SARIF without re-running any server; a failing document
  exits non-zero so rendered reports can gate in their own right.
- `mcpeval serve --print-config`: emits a ready-to-paste MCP client config
  snippet for the agent-loop endpoint.
- Action self-test workflow: the repository consumes its own composite
  action on every push (clean demo passes; a broken demo must fail the
  action), exercising the distribution channel itself.
- Stdio client interoperability hardening from dogfooding against the official
  reference server: stdout banners and unsolicited notifications no longer
  abort a session, while mismatched response ids still fail fast; a server
  that never answers times out instead of echoing prose.

### Security

- Probe calls remain read-only by default and use the existing sanitized
  persistence boundary.
- Unprobeable findings are capped at medium severity, and verification validates
  the finding/case/tool relationship before launching a server process.

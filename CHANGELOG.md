# Changelog

## Unreleased

### Added

- `completion` probe: for a server declaring the `completions` capability,
  one `completion/complete` request for the manifest's reference
  (`ref_type`/`ref_uri`) and argument must answer a well-formed completion
  — `completion.values`, an array of strings — within `max_values`
  (1..=100). A structured error naming the argument is
  `completion-argument-unknown` (the completion surface out of sync with
  the prompt's own declarations); a malformed envelope is
  `completion-invalid-request`; more than `max_values` is
  `completion-value-flood`; a transport failure is
  `completion-stalled-request`. Undeclared support passes trivially.
  Scores under the contract category. The demo server gained a
  `--broken completion` personality (non-string completion values) and
  its `welcome` prompt now declares a `language` argument served by
  `completion/complete`. Manifest JSON schema, remediation hints, and
  official docs cover the probe.
- Corpus drift check: `node scripts/corpus/verify.mjs` re-probes every
  observation in `data/readiness-corpus.json` with the current binary and
  exits non-zero on any score that moved; a `corpus-drift` workflow runs it
  weekly and on pull requests touching the corpus or the probe battery, so
  "reproducible by anyone" stays an enforced property rather than a claim.
  Agreement tests pin the drift check's launch commands to the collector's
  arrays.
- Corpus grew from 19 to 34 observations: `data/readiness-corpus.json` now
  holds 34 public servers collected with `scripts/corpus/collect.sh` —
  fifteen additional credential-free servers across the npm and uvx
  ecosystems (airbnb, sqlite, docker, mermaid, terraform, tavily, ollama,
  calculator, wikipedia, searxng, git, arxiv). Every prior observation
  reproduced byte-identically on re-run. With 34 observations the
  readiness percentile resolves on ~3% steps instead of ~5%.

### Changed

- `record_annotation` MCP tool on `mcpeval serve`: the write-side agent
  tool. Records an agent-authored observation about a captured call,
  identified by `(session, seq)`, through the identical build → validate →
  `append_annotation` path the `mcpeval annotate` command uses — fixed
  kind set, 240-character bounded control-character-free note, session
  hashed before persistence. The agent loop is now fully native: scaffold,
  probe, read verdicts, record observations, re-run.
- `mcpeval diff <BASELINE> <CURRENT>`: native baseline gating. Compares two
  committed `mcpeval.probe-report/v1` documents, matches cases by id and
  probe kind, and classifies every case as regressed, fixed, or unchanged.
  `--fail-on-regression` exits non-zero only for regressions — fixes and
  manifest growth (removed/added cases) are informational. Readiness
  movement is printed alongside per-case measurement movement (catalog
  tokens, slowest latency). `--format json` emits a deterministic,
  versioned `mcpeval.probe-diff/v1` document; `--format markdown` renders a
  pull-request-ready movement table; either path accepts `-` for stdin.

## 0.2.0 - 2026-09-10

### Added

- Four 2025-06-18 spec-surface probes:
  `protocol-negotiation` (a fresh handshake with the supported version is
  echoed, an unknown date-shaped version is answered with a date-shaped
  non-echoed version, and the claimed version is itself echoable on a
  third handshake),
  `sampling` (a tool call under a client `sampling` capability; the server's
  `sampling/createMessage` sub-requests are answered with a stub sample,
  bounded by `max_requests`),
  `elicitation` (same shape for `elicitation/create`, answered with the
  manifest's `respond` action), and
  `resource-subscription` (for a server declaring `resources.subscribe`:
  read, subscribe, trigger, and require `notifications/resources/updated`
  within `max_wait_seconds`, then unsubscribe cleanly; undeclared support
  passes trivially). Negotiation, sampling, and elicitation score under the
  contract category; resource-subscription under reliability. The demo
  server gained `--broken negotiation`, `--broken sampling`,
  `--broken elicitation`, and `--broken subscription` personalities and
  `sampled_read`, `elicited_read`, and `publish_status` tools.
- `cancellation` probe: issues a read-only call, cancels it with
  `notifications/cancelled`, and requires the server to acknowledge the
  cancellation — silence for the cancelled request id within `grace_seconds`
  or the structured "Request cancelled" error (-32800) that production
  servers return; preflight confirms the tool succeeds uncancelled. Works on
  stdio and Streamable HTTP (the call POST runs on its own connection with
  `grace_seconds` as its timeout, so the cancellation lands mid-flight). The
  demo server gained a `--broken cancellation` aspect that drops the
  notification, and its slow tool now honors cancellation through a
  dedicated stdin reader thread.
- `mcpeval compare` accepts one stdio command after `--` as an additional
  comparison column labeled `stdio`, alongside `--endpoint` HTTP targets;
  two or more targets are required.
- State of MCP servers guide: the readiness corpus
  (`data/readiness-corpus.json`) holds 19 public servers collected with
  `scripts/corpus/collect.sh`, and the distribution is published with its
  method notes.
- npm package `@cavi-ai/mcp-eval` and Homebrew formula `Formula/mcpeval.rb`,
  both pinned to the v0.1.0 release archives and SHA-256 digests recorded in
  `distribution/release.json`; the npm installer verifies the checksum
  companion, the pinned archive size, and the SHA-256 before extracting the
  binary.
- CI: distribution contract check, `npm install` end-to-end on Linux and
  Windows, and Homebrew audit, install, and test on macOS.

### Changed

- npm publication uses trusted publishing (OIDC) with provenance; no npm
  token is stored in the repository.

## 0.1.0 - 2026-08-30

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

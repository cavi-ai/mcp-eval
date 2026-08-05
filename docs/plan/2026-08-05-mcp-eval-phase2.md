# Phase 2 Promotion and Findings Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Aggregate indexed MCP calls into scored issues, enforce evidence-based promotion, calibrate the default threshold, and emit privacy-safe findings reports.

**Architecture:** A focused `promote` module reads the existing SQLite index, computes deterministic issue aggregates, and transactionally replaces derived issue/finding tables. A `report` module projects only privacy-safe finding fields into agent, Markdown, and JSON formats; CLI commands supply configuration and time boundaries.

**Tech Stack:** Rust 2021, rusqlite, chrono, serde/serde_json, clap, Cargo integration tests.

## Global Constraints

- Preserve JSONL as the immutable source of truth and keep derived state rebuildable.
- Never report raw templates, annotation notes, session identifiers, salt, paths, or unredacted values.
- Promotion always requires at least two distinct failing sessions.
- The threshold is configurable and defaults to the value calibrated from a synthetic seed corpus.
- Modify only Phase 2 implementation, tests, fixtures, and directly related documentation.

---

### Task 1: Scoring and Seed Calibration

**Files:**
- Create: `src/promote.rs`
- Create: `tests/promote.rs`
- Create: `tests/fixtures/phase2-seed.json`
- Modify: `src/lib.rs`

**Interfaces:**
- Produces: `wilson_lower_bound(failures: u64, calls: u64) -> anyhow::Result<f64>`
- Produces: `score(input: ScoreInput) -> anyhow::Result<ScoreParts>`
- Produces: `calibrate_seed() -> anyhow::Result<f64>` and `DEFAULT_THRESHOLD: f64`

- [ ] **Step 1: Write failing scoring tests** for the hand-calculated Wilson references `(2,2) = 0.3423719529` and `(40,100) = 0.3094012864`, fourteen-day recency halving, score monotonicity, future-time clamping, invalid counts, and seed-class separation.
- [ ] **Step 2: Run `cargo test --test promote scoring -- --nocapture`** and verify failure because `mcpeval::promote` does not exist.
- [ ] **Step 3: Implement minimal pure scoring and calibration code**, parsing the checked-in aggregate fixture with blocker/annoyance labels and choosing the midpoint between class boundaries.
- [ ] **Step 4: Run `cargo test --test promote scoring -- --nocapture`** and verify all scoring tests pass.

### Task 2: Indexed Issue Aggregation and Promotion

**Files:**
- Modify: `src/promote.rs`
- Modify: `tests/promote.rs`

**Interfaces:**
- Produces: `PromotionConfig { threshold: f64, now: DateTime<Utc> }`
- Produces: `promote(root: &Path, config: PromotionConfig) -> anyhow::Result<PromotionStats>`
- Materializes: `issues` and `findings` SQLite tables.

- [ ] **Step 1: Write failing integration tests** that build real journals/indexes and assert complete-key grouping, per-server/tool denominators, median real-turn cost, distinct-tool blast radius, deterministic recency, and transactional replacement.
- [ ] **Step 2: Run `cargo test --test promote aggregation -- --nocapture`** and verify failures are due to absent aggregation.
- [ ] **Step 3: Implement aggregation against `calls`, `windows`, and `annotations`**, retaining only safe shapes and opaque identifiers, then materialize issues and findings in one transaction.
- [ ] **Step 4: Write and run a failing single-session test** proving a high score and zero threshold still do not promote.
- [ ] **Step 5: Implement the unconditional two-session predicate** and verify `cargo test --test promote` passes.

### Task 3: Configuration and Promote CLI

**Files:**
- Modify: `src/cli.rs`
- Modify: `src/main.rs`
- Modify: `src/promote.rs`
- Modify: `tests/cli.rs`

**Interfaces:**
- Adds: `mcpeval promote [--threshold <number>]`
- Reads: `<MCPEVAL_HOME>/config.json` field `promotion_threshold`

- [ ] **Step 1: Write failing CLI tests** for calibrated default, config-file value, CLI precedence, invalid/negative/non-finite values, and printed issue/finding counts.
- [ ] **Step 2: Run `cargo test --test cli promote -- --nocapture`** and verify clap rejects the missing command.
- [ ] **Step 3: Implement threshold validation/resolution and the CLI command** without creating a config file or leaking its contents.
- [ ] **Step 4: Run `cargo test --test cli promote -- --nocapture`** and verify all promote CLI tests pass.

### Task 4: Privacy-Safe Findings Reporter

**Files:**
- Create: `src/report.rs`
- Create: `tests/report.rs`
- Modify: `src/lib.rs`
- Modify: `src/cli.rs`
- Modify: `src/main.rs`

**Interfaces:**
- Produces: `ReportFormat::{Agent, Md, Json}` and `render(root: &Path, format: ReportFormat) -> anyhow::Result<String>`
- Adds: `mcpeval findings --format agent|md|json`

- [ ] **Step 1: Write failing report tests** for deterministic ordering, required metrics, JSON validity, Markdown/agent rendering, actionable missing-promotion error, and severity annotation uplift.
- [ ] **Step 2: Add privacy canaries** in raw templates, notes, sessions, paths, and argument values; assert none occur in any report while sanitized shape tokens do.
- [ ] **Step 3: Run `cargo test --test report -- --nocapture`** and verify failure because the reporter does not exist.
- [ ] **Step 4: Implement the safe database projection and three renderers**, never selecting raw template, note, or session columns.
- [ ] **Step 5: Add the findings CLI command** and verify `cargo test --test report --test cli` passes.

### Task 5: Documentation and Completion Verification

**Files:**
- Modify: `README.md`
- Modify: `docs/install.md`

**Interfaces:**
- Documents: promotion prerequisites, threshold precedence, calibration, two-session rule, and findings formats/privacy boundary.

- [ ] **Step 1: Update user documentation** with exact Phase 2 commands and safe sharing guidance.
- [ ] **Step 2: Run `cargo fmt -- --check`** and fix only Phase 2 formatting issues.
- [ ] **Step 3: Run `cargo test --all-targets`** and resolve every failure without weakening assertions.
- [ ] **Step 4: Run `cargo clippy --all-targets -- -D warnings`** and resolve every warning.
- [ ] **Step 5: Run `git diff --check` and inspect `git diff --stat`/`git status --short`** to confirm only intended files changed.

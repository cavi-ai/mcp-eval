# Phase 2 Promotion and Findings Design

## Scope

Phase 2 turns the privacy-safe SQLite call index into aggregated issues, promotes
well-supported issues into findings, calibrates the default promotion threshold
against a checked-in synthetic seed corpus, and reports findings in agent, Markdown,
or JSON form. Probe generation and finding lifecycle transitions remain Phase 3 and
Phase 4 work.

## Architecture

The new promotion module reads `index.db` after indexing and groups error calls by the
existing issue identity `(server, tool, err_code, err_template_id)`. It writes
rebuildable `issues` and `findings` tables in the same database. JSONL remains the
source of truth; promotion never mutates captured records.

Each aggregate contains failure and relevant call counts, distinct failing sessions,
the most recent occurrence, median window cost, blast radius, score components, and
the final score. A finding is an issue whose score meets the active threshold and
whose failures span at least two sessions.

## Issue Metrics

For an issue, the denominator is all `tools/call` rows with the same server and tool;
the numerator is error rows matching the complete issue identity. This makes the rate
meaningful for a specific failure mode without combining unrelated templates.

The score is:

```text
rate       = failures / calls
confidence = 95% Wilson lower bound of rate (z = 1.959963984540054)
recency    = 0.5 ^ (age_days / 14)
cost       = median count of agent turns in each failure window
blast      = distinct non-null tools appearing across those windows
score      = confidence * recency * log2(1 + cost) * (1 + log2(blast))
```

An agent turn is a distinct neighboring real call in the indexed failure window.
Synthetic calls do not increase cost. The failing call itself contributes one turn,
so a failure with no neighbors still has non-zero cost. Blast radius has a floor of
one and includes the failing tool, preventing undefined logarithms.

Future timestamps have zero age. Invalid timestamps fail promotion rather than being
silently treated as recent. Empty denominators and impossible aggregate states are
errors.

## Promotion and Configuration

`mcpeval promote` rebuilds issue and finding rows transactionally. It accepts an
optional `--threshold`; otherwise it reads `promotion_threshold` from
`<MCPEVAL_HOME>/config.json`; when the file or field is absent, it uses the calibrated
default. Thresholds must be finite and non-negative.

The two-session rule is unconditional. No score, annotation, or threshold override can
promote an issue observed in fewer than two distinct failing sessions.

The seed corpus is a checked-in synthetic fixture containing representative blocker
and annoyance aggregates derived from the seventeen documented observations. It
contains no captured payloads, notes, session identifiers, or source error text.
Calibration chooses the midpoint between the highest annoyance score and lowest
blocker score. Calibration fails if the classes overlap. A focused test pins the
result and proves every blocker promotes while every annoyance remains below the
threshold.

## Findings Report

`mcpeval findings --format agent|md|json` reads the materialized findings ordered by
descending score with stable issue-key tie-breaking. Reports include only:

- server and sanitized tool name;
- opaque error code and salted template identifier, when present;
- failure/call/session counts and last occurrence;
- cost, blast radius, score components, score, threshold, and derived severity;
- a shape-level reproduction assembled only from already-redacted indexed argument
  shapes.

Reports never include raw error templates, annotation notes, raw session values,
fingerprint salt, store paths, or unredacted request data. Severity begins at low,
medium, or high based on score bands relative to the active threshold and moves up one
band when linked annotations include `false-success` or `blocked-optimal-path`.
Annotation prose is never copied into a finding.

The agent format is compact structured text, Markdown is human-readable, and JSON is
a stable serializable array. If promotion has not run, findings returns an actionable
error instead of an empty success.

## Failure Handling

Promotion requires a current `index.db` with the expected Phase 1.5 schema. Database
updates use one transaction so prior derived results survive failed rebuilds. Reports
validate their format through clap and surface schema or data errors with context.

## Testing

Focused unit and integration tests cover:

- Wilson lower-bound reference values and monotonicity in successes;
- recency decay, future timestamps, median cost, and blast radius;
- complete issue-key grouping and the call denominator;
- unconditional two-session enforcement;
- configuration precedence and invalid thresholds;
- separating seed blockers from single-occurrence annoyances;
- deterministic report ordering and all three formats;
- report privacy boundaries, including planted raw templates, notes, sessions, and
  argument values that must never appear.

The full existing test suite and formatting checks remain the completion gate.

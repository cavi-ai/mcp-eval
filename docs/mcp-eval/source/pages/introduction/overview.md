# MCP Eval

MCP Eval captures the operational cost that MCP servers impose on agents and turns repeated friction into privacy-safe, queryable findings. It can proxy stdio or Streamable HTTP traffic, index sanitized records, promote recurring failures, and run deterministic probes against declared server behavior.

## Headline evaluation dimensions

Version {{PRODUCT_VERSION}} has exactly five headline evaluation dimensions:

- `discovery-cost`
- `schema-guessability`
- `error-honesty`
- `state-recovery`
- `contention`

## Supplemental probes

The CLI also exposes five supplemental probes. They are useful deterministic checks, but they are not additional headline evaluation dimensions for this release:

- `token-cost`
- `degradation-over-n`
- `instruction-fidelity`
- `latency-budget`
- `pagination`

The system is read-only by default. A mutating probe needs both a declared sandbox in a strict version 1 manifest and an explicit `--allow-mutation` flag. Capture and probe output follows the same privacy boundary: raw request values, response bodies, tool descriptions, and raw error prose are not persisted.

The finding lifecycle is evidence-driven. A finding starts open, moves through verification, closes after three consecutive green verification runs, and reopens when a later verification is red.

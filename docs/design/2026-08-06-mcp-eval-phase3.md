# Phase 3 Probe Battery Design

## Scope

Phase 3 adds a local stdio probe runner, the `degradation-over-n` and
`instruction-fidelity` probes, and a strict versioned manifest that makes mutation
explicit. Generated probes, HTTP/SSE transport, LLM judging, and finding lifecycle
automation remain future work.

## Execution Model

`mcpeval probe` validates the complete manifest and selected cases before starting a
server. It performs MCP initialization and tool discovery, then calls only tools named
by selected cases. `degradation-over-n` repeats one call up to `max_attempts` (2–100)
and reports the first error. `instruction-fidelity` makes one call and compares the
outcome with declared top-level fields, bounded scalar values, or an integer error
code.

Fidelity evaluation is deliberately deterministic. No tool description, argument,
or response is sent to an external judge. A semantic LLM judge requires a separate
provider and disclosure policy before it can be added safely.

## Manifest and Mutation Safety

The root `version` is exactly `1`; all root, sandbox, case, and expectation objects
reject unknown fields. Identifiers, tool names, descriptions, attempt bounds,
arguments, and outcome-specific expectations are validated. Cases are ordered and
IDs are unique.

Read-only cases cannot name a sandbox. A mutating case must name a sandbox declared
in the root map, and the operator must independently pass `--allow-mutation`. Missing
or invalid manifests, undefined sandboxes, and missing operator opt-in fail before
process launch. This makes mutation a two-party decision between server author and
operator rather than an inference from tool metadata.

## Privacy Boundary

Each attempted tool call becomes an ordinary sanitized `CallRecord` tagged
`synthetic`. The store retains opaque session, bounded server/tool/method labels,
latency, shaped arguments, outcome, and sanitized error metadata. It never retains
manifest case IDs, sandbox declarations, raw manifest arguments, tool descriptions,
response bodies, or error prose.

CLI output contains only validated case IDs, probe names, attempt counts,
first-failure indices, and fixed failure reasons. Protocol errors do not echo raw
frames. Child stderr is suppressed so server payloads cannot bypass this boundary.
The manifest itself is not share-safe even though its contents are not persisted.

## Testing

Unit tests cover strict schema and cross-field validation, synthetic-record
sanitization, authorization preflight, and fixed fidelity reasons. Real clean and
broken Python stdio fixtures exercise initialization, discovery, calls, degradation,
fidelity divergence, malformed frames, mismatched IDs, early exit, child-launch
prevention, mutation gates, and planted privacy canaries.

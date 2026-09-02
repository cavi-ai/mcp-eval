# Evaluation dimensions

MCP Eval {{PRODUCT_VERSION}} has exactly five headline evaluation dimensions. The CLI also exposes nine supplemental probes, documented separately below.

## Headline evaluation dimensions

### `discovery-cost`

Measures the number of tools and encoded `tools/list` catalog bytes in memory. The case passes when both values stay within `max_tools` and `max_schema_bytes`. Catalog descriptions and schemas are not persisted or printed.

### `schema-guessability`

Requires the selected tool to expose an object input schema. Every required field must exist in `properties` and in the manifest's naive arguments, and the resulting tool call must succeed.

### `error-honesty`

Requires an initial error, a stable error code, and `retryable` metadata matching `expect_retryable`. A retryable error must recover within `max_attempts`; a non-retryable error must remain consistently non-retryable through the confirmation attempt.

### `state-recovery`

Runs the declared failure, recovery, and validation calls in order. It passes only when failure is observed, recovery succeeds, and validation succeeds. Mutating recovery plans need a declared sandbox and `--allow-mutation`.

### `contention`

Synchronizes two independently initialized MCP clients against the same tool and arguments. Both calls must succeed.

## Supplemental probes

These checks are available in the manifest and CLI, but are not additional headline evaluation dimensions in version {{PRODUCT_VERSION}}.

### `token-cost`

Estimates the context cost of the `tools/list` catalog. Each complete tool entry
(name, description, and input schema) is measured in memory as encoded bytes,
then divided by four and rounded up. The deterministic, model-independent
estimate is checked against `max_total_tokens` and an optional
`max_tool_tokens` ceiling. Only tool names and numeric measurements appear in
the result; descriptions and schemas are never persisted or printed.

### `degradation-over-n`

Repeats the declared tool call through `max_attempts`. The first error fails the case and records its attempt number; all calls must succeed to pass.

### `instruction-fidelity`

Runs one call and compares its machine-readable outcome with the declared `expect` object. It can check required result fields, exact scalar values, or an error code. It does not invoke an external model.

### `latency-budget`

Runs a declared read-only tool through a bounded number of attempts and fails
when an observed response exceeds `max_latency_ms`. It reports only the
numeric latency measurement and a fixed failure label.

### `pagination`

Walks a read-only `tools/list` catalog through its declared cursor sequence.
It fails on malformed pages, duplicate tools, repeated cursors, or an
exceeded `max_pages` bound. Catalog descriptions and schemas stay in memory.

### `payload-bounds`

Injects one exact-size argument — a deep copy of the declared arguments with a
single oversized ASCII string field — and verifies the server neither crashes
nor hangs. A clean structured JSON-RPC rejection is honest bounded behavior
unless `expect_handled` asserts the tool must actually handle the size, in
which case a rejection fails the case. A transport-level failure always fails
with `payload-unhandled`. Only the numeric latency and a fixed label are
reported; the injected bytes are never echoed.

### `surface-listing`

Validates the envelopes of the server's declared `resources` and `prompts`
surfaces: item arrays must exist, cursor sequences must terminate within
`max_pages`, and listing errors are defects. Surfaces the server did not
declare pass trivially; the probe only verifies what the server claims.

### `output-schema`

For a tool that declares `outputSchema`, the response must carry
`structuredContent` covering that schema's required fields. Tools without a
declared output schema pass trivially. The probe never inspects values, only
field presence.

### `cancellation`

Issues a read-only call, cancels it with `notifications/cancelled`, and
requires the server to acknowledge the cancellation: either silence for the
cancelled request id within `grace_seconds`, or the structured "Request
cancelled" error (`-32800`) that production servers return for cancelled
requests. The probe first confirms the tool succeeds uncancelled, so an
honoring server cannot pass by accident. A full result delivered after the
cancellation, or an unrelated error code, fails the case on both stdio and
Streamable HTTP transports.

All tool calls pass through the normal sanitized synthetic-record boundary.

# Evaluation dimensions

MCP Eval {{PRODUCT_VERSION}} has exactly five headline evaluation dimensions. The CLI also exposes two supplemental probes, documented separately below.

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

### `degradation-over-n`

Repeats the declared tool call through `max_attempts`. The first error fails the case and records its attempt number; all calls must succeed to pass.

### `instruction-fidelity`

Runs one call and compares its machine-readable outcome with the declared `expect` object. It can check required result fields, exact scalar values, or an error code. It does not invoke an external model.

All tool calls pass through the normal sanitized synthetic-record boundary.

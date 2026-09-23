# Manifest reference

Probe manifests are strict JSON documents. Unknown fields are rejected.

```json
{
  "version": 1,
  "sandboxes": {
    "fixture": { "description": "disposable test state" }
  },
  "probes": [
    {
      "id": "bounded-discovery",
      "probe": "discovery-cost",
      "access": "read_only",
      "max_tools": 10,
      "max_schema_bytes": 1000
    },
    {
      "id": "parallel-read",
      "probe": "contention",
      "tool": "read_status",
      "access": "read_only",
      "arguments": {}
    }
  ]
}
```

Top-level `version` must be `1`, `probes` must contain at least one case, and case IDs must be unique valid identifiers. `sandboxes` defaults to an empty object. The optional `timeout_ms` (100 to 600000) is how long each request waits for its response. Without it, stdio waits 30 seconds and HTTP 5 seconds. A `latency-budget` call waits `max_latency_ms` plus that timeout, so a slow call is measured against its budget instead of timing out. A request past its timeout marks the case `transport-timeout`. Sandbox descriptions are bounded to 240 characters and cannot contain control characters.

Every case declares `access` as `read_only` or `mutating`. A read-only case must not name a sandbox. A mutating case must name a key declared in `sandboxes` and still requires `--allow-mutation` at execution time.

Headline evaluation dimension fields:

- `contention`: `tool`, object `arguments`.
- `error-honesty`: `tool`, object `arguments`, `max_attempts` from 2 through 20, and boolean `expect_retryable`.
- `state-recovery`: `failure_tool`, `failure_arguments`, `recovery_tool`, `recovery_arguments`, `validation_tool`, and `validation_arguments`; every arguments value is an object.
- `discovery-cost`: read-only only, with `max_tools` from 1 through 10,000 and `max_schema_bytes` from 1 through 10,000,000.
- `schema-guessability`: `tool` and object `arguments`.

Supplemental probe fields:

- `token-cost`: read-only only, with `max_total_tokens` from 1 through 1,000,000 and optional `max_tool_tokens` from 1 through 100,000. A per-tool limit cannot exceed the total limit.
- `degradation-over-n`: `tool`, object `arguments`, and `max_attempts` from 2 through 100.
- `instruction-fidelity`: `tool`, object `arguments`, and `expect`.
- `latency-budget`: read-only only, with `tool`, object `arguments`, `attempts` from 1 through 20, and `max_latency_ms` from 1 through 60,000.
- `pagination`: read-only only, with `max_pages` from 1 through 100.
- `payload-bounds`: read-only only, with `tool`, object `arguments`, identifier `field`, `size_bytes` from 1 through 16,000,000, and boolean `expect_handled`.
- `surface-listing`: read-only only, with `max_pages` from 1 through 1,000.
- `output-schema`: read-only only, with `tool` and object `arguments`.
- `cancellation`: read-only only, with `tool`, object `arguments`, `grace_seconds` from 1 through 60 (how long the probe waits after cancelling for a response the server must never send), and an identifier `reason` recorded in the `notifications/cancelled` notification.
- `protocol-negotiation`: read-only only, with a date-shaped `bogus_version` (YYYY-MM-DD) other than the supported protocol version.
- `sampling`: read-only only, with `tool`, object `arguments`, and `max_requests` from 1 through 10.
- `elicitation`: read-only only, with `tool`, object `arguments`, `max_requests` from 1 through 10, and `respond` set to `accept`, `decline`, or `cancel`.
- `resource-subscription`: read-only only, with `uri` (1–512 characters), optional `trigger_tool` and object `trigger_arguments`, and `max_wait_seconds` from 1 through 60.
- `completion`: read-only only, with `ref_type` (`ref/prompt` or `ref/resource`), `ref_uri` (1–512 characters), `argument_name`, identifier-shaped `argument_value`, and `max_values` from 1 through 100.

An `expect` object declares `outcome` as `ok` or `error`. Successful expectations may include unique `required_result_fields` and scalar `equals` values. Error expectations may include a numeric `error_code`, but cannot declare result fields. This probe checks declared machine-readable result fields, values, outcomes, and error codes; it does not send descriptions or results to an external LLM.

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
      "id": "repeat-read",
      "probe": "degradation-over-n",
      "tool": "read_status",
      "access": "read_only",
      "arguments": {},
      "max_attempts": 5
    }
  ]
}
```

Top-level `version` must be `1`, `probes` must contain at least one case, and case IDs must be unique valid identifiers. `sandboxes` defaults to an empty object. Sandbox descriptions are bounded to 240 characters and cannot contain control characters.

Every case declares `access` as `read_only` or `mutating`. A read-only case must not name a sandbox. A mutating case must name a key declared in `sandboxes` and still requires `--allow-mutation` at execution time.

Probe-specific fields:

- `contention`: `tool`, object `arguments`.
- `error-honesty`: `tool`, object `arguments`, `max_attempts` from 2 through 20, and boolean `expect_retryable`.
- `state-recovery`: `failure_tool`, `failure_arguments`, `recovery_tool`, `recovery_arguments`, `validation_tool`, and `validation_arguments`; every arguments value is an object.
- `discovery-cost`: read-only only, with `max_tools` from 1 through 10,000 and `max_schema_bytes` from 1 through 10,000,000.
- `schema-guessability`: `tool` and object `arguments`.
- `degradation-over-n`: `tool`, object `arguments`, and `max_attempts` from 2 through 100.
- `instruction-fidelity`: `tool`, object `arguments`, and `expect`.

An `expect` object declares `outcome` as `ok` or `error`. Successful expectations may include unique `required_result_fields` and scalar `equals` values. Error expectations may include a numeric `error_code`, but cannot declare result fields. This probe checks declared machine-readable result fields, values, outcomes, and error codes; it does not send descriptions or results to an external LLM.

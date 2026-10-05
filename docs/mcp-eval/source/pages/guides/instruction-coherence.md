# Instruction coherence

An agent can follow valid instructions and still reach a tool its session does
not expose. `mcpeval guidance` compares server instructions with fresh tool
catalogs under five client capability profiles: `none`, `roots`, `sampling`,
`elicitation`, and `all` (roots, sampling, and elicitation together).

```sh
mcpeval guidance --server example -- your-mcp-server --flags
mcpeval guidance --server example --format json --url http://127.0.0.1:8080/mcp
```

Discovery sends only `initialize`, `notifications/initialized`, and paged
`tools/list` requests. It never calls tools, follows server instructions, or
writes capture records. A separate connection is initialized for each selected
profile. These profiles exercise discovery under protocol `2025-06-18`; they
do not test interactive sampling or elicitation execution. Roots discovery
answers `roots/list` with an empty list, exposing no host paths, with at most
eight roots requests per client request. On HTTP, roots requests must arrive
in the discovery POST's SSE stream; a server requiring a separate standing
GET stream cannot complete that profile. Such runs are incomplete, not defects.
Remote HTTP endpoints require HTTPS and `--allow-remote-http`.

## Review candidates and declared requirements

A backtick-delimited tool name in `initialize.instructions` that appears in
another selected profile's catalog, but not the current one, produces a
`referenced-tool-unavailable` review candidate. Each candidate names the
affected profile and the profiles in which the tool was actually available.
The hint asks the maintainer to check the reference and explain any capability
requirement. Correct conditional references can also produce candidates:
prose is not interpreted as a contract, and candidates never fail the gate.

References without backticks, names absent from every selected catalog, tool
descriptions, and the semantic clarity of instructions are not evaluated.
`reference_detection: "inline-code-known-tools"` records this scope. A report
with no candidates does not prove that all guidance is correct.

Use explicit required tools to test a known availability contract:

```sh
mcpeval guidance --server example --profile sampling \
  --require-tool trigger-sampling-request -- your-mcp-server --flags
```

Repeat `--profile` to select profiles and `--require-tool` to declare additional
tools. A required tool must be listed in **every selected profile**. Its absence
produces a `required-tool-unavailable` defect and exit `1`. `passed` in JSON
describes only this required-tool gate. Without declared requirements, a
complete discovery run has `passed: true`, even when review candidates exist.

## Observation window and incomplete discovery

Some servers register conditional tools after `notifications/initialized`.
`--settle-ms` waits before catalog discovery: 1,500 milliseconds by default,
with an allowed range of 0 through 5,000. The report records the selected
window; tools registered later are outside the observation. Set a reviewed
window appropriate to the server before using required-tool checks in CI.

Catalog discovery is bounded to 20 pages and 10,000 tools per profile. A lost
transport, initialization error, malformed entry or cursor, duplicate tool,
or incomplete pagination produces `profile-incomplete`. The report then has
`complete: false` and `passed: null`, emits no candidates or defects, and
exits `3`. Incomplete discovery is not evidence that a tool is unavailable.
Invalid invocation options exit `2` before discovery.

## Safe reports

Text and `mcpeval.guidance-report/v1` JSON reports contain validated server and
tool identifiers, profile labels, fixed reasons and hints, numeric settings,
and instruction-presence flags. Instruction prose, descriptions, schemas,
cursor values, session identifiers, endpoint URLs, and raw server errors stay
out of reports. `mcpeval schema guidance` prints the report's JSON Schema.

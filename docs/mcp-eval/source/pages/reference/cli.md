# CLI reference

MCP Eval {{PRODUCT_VERSION}} installs the `mcpeval` binary.

| Command | Required shape | Purpose |
| --- | --- | --- |
| `shim` | `--server <SERVER> -- <CMD>...` | Proxy an stdio server and capture sanitized metadata. |
| `shim-http` | `--server <SERVER> --listen <LISTEN> --upstream <UPSTREAM>` | Proxy Streamable HTTP capture. `--allow-remote-http` permits an explicitly selected remote HTTPS upstream. |
| `probe` | `--server <SERVER>` plus `[-- <CMD>...]` or `--url <URL>` | Run manifest probes. Supports `--manifest`, `--probe`, `--format text\|json\|markdown`, `--price-per-mtok <USD>` for session-cost interpretation in text and markdown, `--allow-mutation`, and `--allow-remote-http`. |
| `init` | `--server <SERVER>` plus a target | Scaffold a discovery/token-cost manifest from `tools/list`. `--confirm-read-only` additionally permits empty-argument schema checks. |
| `schema` | no options | Print the strict manifest JSON Schema for editor validation. |
| `compare` | `--server <SERVER> --endpoint <LABEL=URL>` twice or more | Compare one manifest across Streamable HTTP endpoints. Supports text, Markdown, and JSON output. |
| `export-issues` | `--dir <DIR>` | Write content-free markdown issue drafts for open promoted findings. |
| `trends` | optional `--last <COUNT>` | Show privacy-safe readiness history from full probe runs. |
| `verify` | `--finding <FINDING> --case <CASE>` plus a target | Run one manifest case and update the finding lifecycle. Supports the same manifest and authorization flags as `probe`. |
| `index` | no options | Load JSONL records and derive failure windows. |
| `promote` | optional `--threshold <THRESHOLD>` | Aggregate indexed failures and promote supported findings. |
| `generate` | `--finding <FINDING> --output <OUTPUT> --confirm-read-only` | Generate an eligible read-only manifest. `--force` replaces an existing output. |
| `findings` | optional `--format agent\|md\|json` | Render sanitized findings; default format is `agent`. |
| `serve` | `--listen <LISTEN>` | Serve findings and trends over a loopback Streamable HTTP MCP endpoint. |
| `annotate` | `--session <SESSION> --seq <SEQ> --kind <KIND> --note <NOTE>` | Record a bounded agent-authored observation. |
| `doctor` | optional `--check-redaction` | Run store-hygiene checks. |
| `share` | `--dir <DIR>` | Package the share-safe envelope: the store records plus a SHARE.md manifest, after a clean redaction sweep. The salt, index databases, and manifests are never copied. Supports `--include-probe-history` and `--force`. |

The default manifest path for `probe` and `verify` is `mcp-eval.manifest.json`. The five headline evaluation dimension values for `--probe` are `contention`, `error-honesty`, `state-recovery`, `discovery-cost`, and `schema-guessability`. The CLI also accepts the supplemental probes `token-cost`, `degradation-over-n`, `instruction-fidelity`, `latency-budget`, `pagination`, `payload-bounds`, `surface-listing`, and `output-schema`; they are not additional headline dimensions in version {{PRODUCT_VERSION}}.

`probe --format json` emits the versioned, deterministic
`mcpeval.probe-report/v1` document. It contains the validated server label,
case IDs, probe names, fixed failure labels, pass state, and numeric
measurements only; it excludes timestamps, sessions, arguments, response
bodies, descriptions, schemas, and raw error text.

Use `mcpeval help <COMMAND>` for the binary's exact invocation text.

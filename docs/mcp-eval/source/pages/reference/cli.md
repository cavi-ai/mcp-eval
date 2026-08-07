# CLI reference

MCP Eval {{PRODUCT_VERSION}} installs the `mcpeval` binary.

| Command | Required shape | Purpose |
| --- | --- | --- |
| `shim` | `--server <SERVER> -- <CMD>...` | Proxy an stdio server and capture sanitized metadata. |
| `shim-http` | `--server <SERVER> --listen <LISTEN> --upstream <UPSTREAM>` | Proxy Streamable HTTP capture. `--allow-remote-http` permits an explicitly selected remote HTTPS upstream. |
| `probe` | `--server <SERVER>` plus `[-- <CMD>...]` or `--url <URL>` | Run manifest probes. Supports `--manifest`, `--probe`, `--allow-mutation`, and `--allow-remote-http`. |
| `verify` | `--finding <FINDING> --case <CASE>` plus a target | Run one manifest case and update the finding lifecycle. Supports the same manifest and authorization flags as `probe`. |
| `index` | no options | Load JSONL records and derive failure windows. |
| `promote` | optional `--threshold <THRESHOLD>` | Aggregate indexed failures and promote supported findings. |
| `generate` | `--finding <FINDING> --output <OUTPUT> --confirm-read-only` | Generate an eligible read-only manifest. `--force` replaces an existing output. |
| `findings` | optional `--format agent\|md\|json` | Render sanitized findings; default format is `agent`. |
| `annotate` | `--session <SESSION> --seq <SEQ> --kind <KIND> --note <NOTE>` | Record a bounded agent-authored observation. |
| `doctor` | optional `--check-redaction` | Run store-hygiene checks. |

The default manifest path for `probe` and `verify` is `mcp-eval.manifest.json`. The five headline evaluation dimension values for `--probe` are `contention`, `error-honesty`, `state-recovery`, `discovery-cost`, and `schema-guessability`. The CLI also accepts the supplemental probes `degradation-over-n` and `instruction-fidelity`; they are not additional headline dimensions in version {{PRODUCT_VERSION}}.

Use `mcpeval help <COMMAND>` for the binary's exact invocation text.

# Privacy and authorization

MCP Eval is read-only by default and has no raw-payload mode. It stores structured call metadata and shaped arguments, not raw response bodies. Human error text is reduced to the constant `{message}` plus a salted template identifier; the original message is not stored.

Server labels, methods, tool names, keys, enum values, numeric and boolean values, and registrable domains are retained only within their documented bounded grammars. Other hosts and string values are reduced to privacy-safe categories or length buckets. Server stderr passes through to the client and is not journaled.

Probe records are tagged `synthetic` and use the same persistence boundary. Raw manifest arguments, response bodies, tool descriptions, sandbox descriptions, and raw errors are not stored or printed in summaries. Manifest files can still contain sensitive operational inputs and are outside the share-safe boundary.

Capture records under `<MCPEVAL_HOME>/store/` are content-minimized, but annotation notes are deliberate free-form user prose and are not automatically redacted. Before sharing any store records, you must manually review or remove every annotation note and inspect the remaining files. The fingerprint salt lives at `<MCPEVAL_HOME>/.salt`, outside `store/`, and must never accompany shared records. Do not share the entire capture root.

Mutation requires two independent controls: the manifest case uses `"access": "mutating"` and names a declared sandbox, and the operator passes `--allow-mutation`. A missing or invalid manifest, undeclared sandbox, or missing flag never authorizes mutation. `generate --confirm-read-only` attests that an eligible tool is read-only and does not authorize mutation.

HTTP endpoints are loopback-only by default. Remote endpoints require HTTPS plus `--allow-remote-http`. Optional authorization is read from `MCPEVAL_HTTP_AUTHORIZATION`, validated, used in memory, and never persisted or printed. The HTTP proxy may relay an incoming `Authorization` value in memory, but it does not originate calls or grant mutation permission. `mcpeval serve` refuses requests without a loopback `Host`, with a non-loopback `Origin`, or without `Content-Type: application/json`, and lists its process-launching tools (`run_probe`, `scaffold`, `score`, `verify_finding`) only with `--allow-spawn`. `mcpeval shim-http` applies the same `Host` and `Origin` checks to every request and the `Content-Type` check to every POST before forwarding anything upstream.

## Evaluation resource limits

Stdio messages are limited to 4 MiB including the newline. The evaluator queues
at most 16 incoming frames and retains at most 128 notifications totaling
4 MiB of encoded data. Exceeding these limits closes the evaluation transport;
reports use the existing transport-error status without including message
contents. The recording shim also refuses frames larger than 4 MiB.

Each evaluator pipe write is bounded by the configured transport timeout.
Response waits use a fixed deadline, so progress messages and unrelated
notifications cannot extend the wait. Client shutdown cancels and joins its
I/O threads and terminates and reaps its direct child process. Descendant
processes are outside this cleanup guarantee.

Standard input synthesis uses a conservative 64 KiB encoded-size budget and a
4096-node budget, with at most 256 items per array built from type constraints,
16 KiB per string built from type constraints, and eight levels of schema
traversal. Values supplied by defaults,
examples, enums, and constants also consume these budgets before cloning.
Schemas exceeding a budget are reported as unsynthesizable rather than called.

## Producing the share envelope

`mcpeval share --dir <directory>` snapshots every selected JSONL file under a shared journal lock, scans the exact exported bytes, and publishes the envelope only after the scan succeeds. Nested JSONL files are checked too; symlinks and output paths overlapping the capture store are refused. A flagged sweep exits `1` and publishes nothing. Invalid JSON records also stop export.

The envelope contains JSONL records, typed annotation metadata, and a `SHARE.md` manifest. Annotation prose is omitted by default; after manual review, `--include-annotation-notes` explicitly includes it and prints a warning. `--include-probe-history` includes readiness-trend history. The redaction scan is a heuristic, not proof that arbitrary metadata is non-sensitive.

The fingerprint salt, databases (including authoritative `lifecycle.db`), and manifests are excluded. Keep any file containing the salt separate. `--force` replaces the entire prior envelope after successful preparation; a refused export preserves it.

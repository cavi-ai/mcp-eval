# Version and support

These pages describe MCP Eval {{PRODUCT_VERSION}}, released from tag `{{RELEASE_TAG}}` at commit `{{RELEASE_COMMIT}}`. They are published at https://cavi-ai.xyz/docs/mcp-eval/; the per-release archive `mcp-eval-docs-{{RELEASE_TAG}}.tar.gz` and its `.sha256` companion remain the byte-identical distribution artifacts.

MCP Eval is under active development. Before 1.0, the CLI, on-disk schema, and manifest format may change. Keep manifests with the release that validates them and review the repository changelog before upgrading.

## Upgrading to 0.8.0

Readiness still uses `mcpeval-standard/2`, with unchanged scoring policy.
CI can gate readiness floors, score regressions, and breaking catalog changes.
Use `--arguments <FILE>` for tools that need working input values and
`--format junit` for test results. See [Continuous integration](../guides/continuous-integration.md).

Cancellation probes still require cancellation to be honored by default. Set
`require_honored: false` for servers that permit an in-flight call to finish;
the probe also checks that the session remains usable. Error probes accept
`data.retryable` as well as the legacy top-level hint.

Regenerate readiness baselines with this evaluator. The bundled corpus was
collected with 0.7.1; its readiness placement requires matching measurement
profiles, including the evaluator version. Its historical measurements are
retained and do not provide a 0.8.0 readiness ranking.

## Upgrading to 0.7.0

Readiness still uses `mcpeval-standard/2`, with unchanged scoring policy.
`mcpeval serve` changes for MCP clients:

- Tool failures return `isError` results whose text carries the message that
  JSON-RPC `-32000` errors carried before.
- Calls naming no listed tool, including evaluation tools without
  `--allow-spawn`, answer `-32602`.
- `list_findings` returns no findings before the first `mcpeval index`.
- The evaluation tools' output schemas list the report's top-level fields;
  `mcpeval schema report` prints the complete schema.

## Upgrading to 0.6.0

Readiness still uses `mcpeval-standard/2`, with unchanged scoring policy.
New reports include measurement profiles. Readiness score deltas and corpus
placement require known, matching profiles, including the evaluator version.
Older reports remain readable, but missing conditions are not inferred.
Use reviewed result assertions to assess semantic correctness.

Finding verification also binds stdio executable and regular file argument
bytes. Changed launch artifacts or evaluator versions restart passing streaks;
existing lifecycle history remains retained. This does not bind the complete
dependency closure or backing service state.

History refreshes validate cached journal bytes and reuse compatible append
records. `mcpeval index --rebuild` forces reconstruction; rerun promotion afterward.
Journals and lifecycle evidence remain intact. Records larger than 4 MiB are
rejected by writers and readers. Doctor checks nested journals and refuses
symlinks and unreadable entries; share export also rejects invalid JSON.

For release calibration, use reviewed target policies and prepared deployment
locks with `--require-deployments`. The bundled corpus remains the historical
standard/1 snapshot; this release supplies no new representative calibration baseline.
See [corpus collection and replay](../guides/corpus-replay.md).

## Upgrading from versions before 0.5.0

Readiness now uses `mcpeval-standard/2`. Scores measured under the older
standard remain historical evidence and are not comparable to new scores.
Case verdicts can still be compared independently of readiness. Review and
regenerate readiness baselines with the new evaluator; the bundled corpus
retains its `mcpeval-standard/1` label and supplies no new-standard score ranking.

Verification history is retained in `lifecycle.db`, independently of the
rebuildable index. A changed evaluator or verification binding restarts the
passing streak; old history does not count as new verification credit.
False-success findings require a reviewed result assertion or expected error,
including when verification uses a handwritten workflow manifest.

Incomplete `tools/list` discovery now blocks scaffolding and ordinary evaluation
with exit 3 before any tool call or manifest write. Pagination probes remain
available to diagnose the listing. An incomplete run supplies neither readiness
nor passing verification credit.

For defects, include the MCP Eval version, sanitized command shape, and only the necessary content-minimized records from `<MCPEVAL_HOME>/store/` after manually reviewing or removing every annotation note. Do not attach the capture root, `.salt`, raw payloads, authorization values, manifests containing operational arguments, or other credentials. Follow the repository security policy for vulnerability reports.

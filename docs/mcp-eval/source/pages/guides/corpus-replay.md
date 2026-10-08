# Corpus collection and replay

Collect a corpus with declared inputs, replay it, and check the saved evidence
before using its verdict in CI. New collections use
`mcpeval.readiness-corpus/v3` and record every target as observed, untested, or
errored. The shipped standard/1 snapshot lacks the provenance required for
replay; collecting today cannot recover its original execution inputs.

Collection, replay, and offline checking are repository scripts. Run them from
a reviewed checkout containing `scripts/corpus`; installing the CLI alone does
not install these scripts. They use Node.js; repository CI uses Node 24. Build
the evaluator and bundled demo from that same checkout:

```sh
cargo build --release --locked --bins
```

Shell examples below use POSIX syntax. On Windows, invoke
`node scripts/corpus/collect.mjs` instead of the shell wrapper and pass
`--binary target/release/mcpeval.exe` to collection and replay, using your
shell's environment-variable syntax.

## Declare inputs and collect

Choose an existing private directory outside the checkout and set its path:

```sh
CORPUS_DIR=/absolute/private/corpus-run
```

Create `targets.json` there. This example illustrates one target's shape;
replace its package, exact version, executable, and arguments with your actual
server. Declare required environment names and service checks instead of
copying empty prerequisite arrays when the server needs them:

For release calibration, prepare the runtime, server, installed dependencies,
and ecosystem lock file in an immutable or read-only deployment directory.
Use the ecosystem's locked installation mode and verify package identity
against the prepared files. Fingerprint that directory, then replace the
example deployment digest below with the printed SHA-256:

```sh
node scripts/corpus/deployment.mjs --root "$CORPUS_DIR/deployment"
```

```json
{
  "schema": "mcpeval.corpus-targets/v1",
  "standard": "mcpeval-standard/2",
  "targets": [{
    "server": "my-server",
    "runtime": "npm",
    "package": "@your-org/mcp-server",
    "version": "1.2.3",
    "bin": "mcp-server",
    "args": [],
    "deployment": {
      "root": "/absolute/private/corpus-run/deployment",
      "executable": "bin/node",
      "entrypoint": "server/dist/index.js",
      "sha256": "0000000000000000000000000000000000000000000000000000000000000000"
    },
    "prerequisites": { "environment": [], "checks": [] }
  }]
}
```

`runtime` can also be `uvx`, or `native` for a locally prepared executable.
Native targets require an explicit deployment tree and SHA-256 lock; they never
fall back to a package runner. Package versions must be exact; floating tags,
ranges, and URLs are refused. Supply credentials at execution time, never in
labels or prerequisite names. Target arguments can contain sensitive inputs,
so keep the target file private.

Calling uses tool annotations unless a reviewed target declares an optional
`assessment` object with both `attest_read_only` and `skip_tools`, for example
`{ "attest_read_only": true, "skip_tools": ["write_record"] }`.
Review every unannotated tool before attesting; exclude unannotated writers by
name. Annotated writers remain forbidden even with attestation. Keep this
policy in the retained target file rather than changing it between collection
and replay.

```sh
scripts/corpus/collect.sh --targets "$CORPUS_DIR/targets.json" \
  --out "$CORPUS_DIR/corpus.json" --require-deployments
```

Collection launches the declared servers and writes the corpus plus original
reports in a directory printed at completion. Existing output is refused by
default. The default minimum is ten observed targets: extend the target list
for that population, or deliberately choose `--min-observations` for a smaller
collection. A candidate below the selected minimum is retained but exits
nonzero; it is not an approved calibration baseline. Meeting the minimum is a
size check, not evidence of representativeness: review the population and
exclusions before accepting a baseline. Completed failing calls
can be observed evidence, while targets that were not exercised remain
untested or errored.

Retain the evaluator binary, target file, corpus, and original reports. Exact
package pins do not lock transitive dependencies, runtime libraries, or backing
service state. The [repository harness reference](https://github.com/cavi-ai/mcp-eval/blob/main/scripts/corpus/README.md)
describes prepared deployment bundles and service-state checks for binding those
declared inputs.

`--require-deployments` rejects any target without a prepared deployment before
launching an evaluator or service. Use it for release collection and replay.
Diagnostic collections can omit it and use package runners, with weaker
dependency identity. A deployment digest binds the declared tree; it does not
prove dependency closure or sandbox execution. System libraries, dynamic
downloads, and commands or imports outside that tree remain external inputs.

## Replay and retain evidence

Set `ORIGINAL_REPORTS` to the actual directory printed by collection; replace
the UUID placeholder below. Keep the original platform and evaluator binary,
and supply the original credentials and service prerequisites:

```sh
ORIGINAL_REPORTS=/absolute/private/corpus-run/corpus.json.reports-UUID
node scripts/corpus/verify.mjs --corpus "$CORPUS_DIR/corpus.json" \
  --targets "$CORPUS_DIR/targets.json" --reports "$ORIGINAL_REPORTS" \
  --out "$CORPUS_DIR/replay-evidence" --require-deployments --json
```

Replay validates original input identities before execution, runs observed
targets again, and compares their measurements under the recorded standard.
All targets must be observed and no area may drift beyond its policy; reliability
has a ten-point tolerance. A pass means replay agreement, not server health:
reproducing a consistently failing server can pass.

New observations retain the report's measurement profile. Collection checks
attestation and tool exclusions against the declared assessment; replay also
requires the original profile. Profile changes are recorded as drift.
Readiness placement uses only observations with the same known profile and
standard. Historical rows without profiles remain readable and can supply
catalog-size context, but supply no readiness placement.

Replay exits `0` for a pass and `1` otherwise, including invalid original inputs.
`--out` reserves a new directory and retains replay reports plus
`verification.json` (`mcpeval.corpus-verification/v1`). Completed failed verdicts
retain their evidence. Fatal errors clean incomplete output when possible;
without `verification.json`, treat the bundle as incomplete. Original inputs
are not copied into the bundle, so retain them separately.

## Check saved evidence offline

The evaluator, deployments, credentials, and backing services are not needed
for this step. Keep the supplied files immutable while checking:

```sh
node scripts/corpus/check-evidence.mjs --evidence "$CORPUS_DIR/replay-evidence" \
  --corpus "$CORPUS_DIR/corpus.json" --targets "$CORPUS_DIR/targets.json" \
  --reports "$ORIGINAL_REPORTS" --json
```

The checker validates original identities and retained report bytes and
reconstructs the replay verdict. It does not launch an evaluator, server,
package runner, health check, or state adapter.

| Exit code | Meaning |
| --- | --- |
| `0` | Intact evidence with a passing replay verdict. |
| `1` | Intact evidence with a failed replay verdict. |
| `2` | Invalid or incomplete evidence; no JSON verdict is emitted. |

Consistency does not authenticate an author or prove that external checks ran.
Manage trusted records of the original inputs and verification artifact
separately. Hashes alone do not protect against a bundle whose reports, hashes,
and summaries were rewritten together.

## Gate CI and retain privately

The [corpus evidence action](continuous-integration.md#saved-corpus-evidence)
uses this offline checker and fails on failed or invalid evidence. Its outputs
and summary contain counts and verdicts rather than private report content.

Targets, original reports, and replay bundles are private artifacts outside
`<MCPEVAL_HOME>/store/` and the `mcpeval share` envelope. Retained report bytes
and extension metadata can contain sensitive content. Use a private parent
with appropriate permissions or ACLs and controlled storage; do not commit
these files or upload them to public workflow artifacts. See
[Privacy and authorization](../security/privacy-and-authorization.md#private-corpus-artifacts).

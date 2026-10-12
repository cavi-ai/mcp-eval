# Corpus collection and replay

Collect a corpus with declared inputs, replay it, and check the saved evidence
before using its verdict in CI. New collections use
`mcpeval.readiness-corpus/v3` and record every target as observed, untested, or
errored. The shipped standard/1 snapshot lacks the provenance required for
replay; collecting today cannot recover its original execution inputs. Historical
`mcpeval.readiness-corpus/v2` data stays readable, but replay refuses it as a
pinned collection. No target list or package version is inferred from
historical observations.

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
shell's environment-variable syntax. `--binary` selects any evaluator
executable; the default is `target/release/mcpeval` in the checkout.

## Declare inputs and collect

Choose an existing private directory outside the checkout and set its path:

```sh
CORPUS_DIR=/absolute/private/corpus-run
```

For release calibration, prepare the runtime, server, installed dependencies,
and ecosystem lock file in an immutable or read-only deployment directory.
Use the ecosystem's locked installation mode and verify package identity
against the prepared files. Fingerprint that directory:

```sh
node scripts/corpus/deployment.mjs --root "$CORPUS_DIR/deployment"
```

Create `targets.json` there. This example illustrates one target's shape;
replace its package, exact version, executable, arguments, and the example
deployment digest with your server's values and the printed SHA-256. Declare
required environment names and service checks instead of copying empty
prerequisite arrays when the server needs them:

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

The [target file reference](#target-file-reference) lists every field.

```sh
scripts/corpus/collect.sh --targets "$CORPUS_DIR/targets.json" \
  --out "$CORPUS_DIR/corpus.json" --require-deployments
```

Collection launches the declared servers and writes the corpus atomically plus
original reports in a directory printed at completion. Existing output is
refused unless `--force` is passed. The default minimum is ten observed
targets: extend the target list for that population, or deliberately choose
`--min-observations` for a smaller collection. A candidate below the selected
minimum is retained but exits nonzero; it is not an approved calibration
baseline. Meeting the minimum is a size check, not evidence of
representativeness: review the population and exclusions before accepting a
baseline. Publishing new measurements remains a separate decision.

The corpus records the evaluator version and executable SHA-256, the target
file and launch hashes, report hashes, package identities, call counts,
prerequisite names, and an [outcome](#target-outcomes) for every target.
Argument and environment values are not copied into it. Only observations
enter calibration.

Retain the evaluator binary, target file, corpus, original reports, and
prepared deployment directories. Exact
package pins do not lock transitive dependencies, runtime libraries, or backing
service state; [prepared deployments](#prepared-deployments) and
[service-state checks](#service-state-checks) bind those declared inputs.

`--require-deployments` rejects any target without a prepared deployment before
launching an evaluator or service. Use it for release collection and replay.
Diagnostic collections can omit it and use package runners, with weaker
dependency identity.

## Target file reference

| Field | Requirement |
| --- | --- |
| `schema` | `mcpeval.corpus-targets/v1`. |
| `standard` | The evaluator's readiness standard, such as `mcpeval-standard/2`. Reports scored under another standard are `invalid-report`. |
| `targets` | Non-empty array. |
| `server` | Unique label: a letter, then up to 127 letters, digits, `_`, `.`, `:`, or `-`. |
| `runtime` | `npm` (runs `npx`), `uvx`, or `native`. Native targets require `deployment` and never fall back to a package runner. |
| `package`, `version`, `bin` | Package name, exact version, and the actual executable name. Tags, ranges, URLs, and wildcards are refused. |
| `args` | Explicit string array, including `[]`. |
| `deployment` | Optional; see [prepared deployments](#prepared-deployments). |
| `assessment` | Optional `{ "attest_read_only": <boolean>, "skip_tools": [<tool names>] }`; both fields are required when present. |
| `prerequisites.environment` | Required environment-variable names, including `[]`: uppercase letters, digits, and `_`, not starting with a digit. `HOME`, `USERPROFILE`, `MCPEVAL_HOME`, `KUBECONFIG`, and `DOCKER_HOST` are refused. |
| `prerequisites.checks` | Health checks, including `[]`: each has a `name` label and a non-empty `command` string array. |
| `prerequisites.state_checks` | Optional; see [service-state checks](#service-state-checks). |

Check names are unique across health and state checks. A check passes when it
exits zero within ten seconds and prints at most 1 KiB. Presence of an
environment variable does not prove its credentials work; declare a health
check when validation is needed. Supply values at execution time; an empty or
whitespace-only value counts as missing. Never put secrets in labels or check
names. Target arguments can contain sensitive inputs, and hashes do not redact
low-entropy secrets, so keep the target file private; a launch hash is not a
credential protection.

Calling uses tool annotations unless a reviewed target declares an
`assessment`, for example
`{ "attest_read_only": true, "skip_tools": ["write_record"] }`.
Review every unannotated tool in the pinned deployment before attesting;
exclude unannotated writers by name. Annotated writers remain forbidden even
with attestation. Never attest merely to raise a score or turn an untested
target into an observation. Keep this policy in the retained target file
rather than changing it between collection and replay.

## Prepared deployments

A `deployment` replaces the package runner with a prepared local bundle. The
collector does not install or resolve packages for it.

| Field | Requirement |
| --- | --- |
| `root` | Absolute path of the prepared directory. |
| `executable` | Relative path of a native runtime executable inside `root`, such as `bin/node`. Do not use a script that obtains an interpreter from the host. |
| `entrypoint` | Optional relative path of the server inside `root`, such as `server/dist/index.js`. A native server omits it. |
| `sha256` | The digest printed by `scripts/corpus/deployment.mjs`. |

The launch command is the absolute executable, the absolute entrypoint when
present, and `args`. `package`, `version`, and `bin` remain the declared
identity; validate them against the bundle when preparing it.

The versioned tree digest covers file names, bytes, permissions, directories,
and relative internal links. External, absolute, dangling, and cyclic links
and special files are refused. Collection and replay check the bundle before
health checks, after each check, and after evaluation; a mismatch is
`deployment-mismatch`. Observations record `deployment_sha256`; absolute paths
stay in the private target file, bound by the launch hash.

Keep the bundle read-only and outside the evaluation home. Hash checks detect
changes; they do not prevent a concurrent writer from changing and restoring
files between checks. A read-only filesystem or immutable image mitigates that
race. A deployment digest does not prove dependency closure or sandbox
execution: system libraries, dynamic downloads, commands found through `PATH`,
and imports outside the tree remain external inputs.

## Service-state checks

`prerequisites.state_checks` binds backing-service state. Each entry has a
`name` label, a non-empty `command` string array, and the expected lowercase
SHA-256 in `sha256`. Capture the expected digest when preparing the service
snapshot; collection does not establish or update it.

A state adapter reads the actual backing service and prints only its stable
state digest, optionally followed by one newline, then exits zero. It runs with
the target's environment and evaluation home, a ten-second deadline, and a
1 KiB output limit. Store adapters in the prepared deployment to bind their
bytes too.

After health checks, collection and replay compare every state digest before
launching the evaluator and again after evaluation, even when evaluation
failed. A different digest is `state-mismatch`. A failed, unavailable, or
malformed check is `state-check-failed`: untested before evaluation, errored
after it. Observations retain only check names and expected digests; adapter
output is not persisted.

Fingerprint the service identity and relevant dataset or configuration
together, and keep credentials and low-entropy values out of digests. A digest
proves equality of what the adapter measures; an adapter that prints a constant
proves nothing. State checks do not restore, freeze, or authenticate a service,
and changes reverted between checks stay undetectable. Use a read-only snapshot
or a service-supported consistent read boundary shared by the server and the
adapter.

## Execution environment

Every target runs in a fresh home shared by its checks and evaluation: `HOME`
and `USERPROFILE` point to it and `MCPEVAL_HOME` to a directory inside it. Only `PATH`, `SystemRoot`,
`WINDIR`, `COMSPEC`, `PATHEXT`, `TMPDIR`, `TEMP`, `TMP`, `LANG`, `LC_ALL`, and
the declared environment pass through. `KUBECONFIG` points to an empty
configuration and `DOCKER_HOST` to an unreachable socket. This is environment
isolation, not a security sandbox.

Each evaluation has a 350-second process deadline and an 8 MiB output limit;
exceeding either terminates the process tree. The evaluator executable's
SHA-256 is checked before every target and at the end; a change stops the run.

## Target outcomes

| Status | Reason | Report kept | Meaning |
| --- | --- | --- | --- |
| observed | | yes | The report has tools and at least one attempted call. |
| untested | `missing-environment` | no | A declared variable is unset or blank; nothing ran. |
| untested | `prerequisite-failed` | no | A health check failed; the server was not launched. |
| untested | `no-tools` | yes | The server listed no tools. |
| untested | `no-tool-calls` | yes | No tool call was attempted. |
| untested or errored | `state-check-failed` | no | A state adapter failed: untested before evaluation, errored after it. |
| errored | `state-mismatch` | no | A state digest differed from the declared one. |
| errored | `deployment-mismatch` | no | The bundle changed or no longer matches its digest. |
| errored | `evaluation-failed` | no | The evaluator timed out, overflowed, failed to launch, or exited other than `0` or `2`. |
| errored | `invalid-report` | no | The report was malformed or did not match this target, evaluator, standard, and assessment. |
| errored | `readiness-unmeasured` | yes | The report carries no readiness score. |

Completed tool failures are observed evidence, including when every attempted
call failed.

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

Replay validates original input identities and report projections before
execution, runs observed targets again, and compares their measurements under
the recorded standard. All targets must be observed and no area may drift
beyond its policy; reliability has a ten-point tolerance. Equal report digests
are not required. A pass means replay agreement, not server health or the
cause of any drift: reproducing a consistently failing server can pass.
Credentials and the host operating system remain external inputs; control
them before attributing drift to a server change.

New observations retain the report's measurement profile. Collection checks
attestation and tool exclusions against the declared assessment; replay also
requires the original profile. Profile changes are recorded as drift.
Readiness placement uses only observations with the same known profile and
standard. Historical rows without profiles remain readable and can supply
catalog-size context, but supply no readiness placement.

Replay exits `0` for a pass and `1` otherwise. Invalid inputs or original
artifact provenance abort before execution with an error on stderr and no
verification artifact. `--json` prints the `mcpeval.corpus-verification/v1`
artifact; `mcpeval schema corpus-verification` prints its schema. It binds the
corpus and target-file bytes, evaluator identity, standard, platform, and
tolerance, and lists each target with expected and observed scores, moved
areas, and original and replay report digests. Originally unobserved targets
have null scores and digests; a missing or discarded replay report has a null
replay digest. Without `--json`, replay prints one line per target.

Without `--out`, the artifact names replay report digests but no report bytes
are kept. `--out` works with either output mode and keeps the replay exit
status. It reserves a new directory under an existing private parent before
any execution and refuses an existing path; there is no force mode. It retains:

```text
replay-evidence/
  verification.json
  reports/<server>.json
```

`reports/` holds every replay report with a non-null digest, including valid
reports from unobserved outcomes. On POSIX, directories are owner-only and
files owner read/write; on Windows, permissions follow the parent's ACLs.
Reports are written first, then `verification.json` is published atomically
as the completion marker; completed failed verdicts keep their bundle. Fatal
errors remove the incomplete bundle when possible. A crash can leave partial
output: without `verification.json`, treat the bundle as incomplete. Do not
modify the directory during replay. Original inputs are not copied into the
bundle, so retain them separately.

## Check saved evidence offline

The evaluator, deployments, credentials, and backing services are not needed
for this step. Keep the supplied files immutable while checking:

```sh
node scripts/corpus/check-evidence.mjs --evidence "$CORPUS_DIR/replay-evidence" \
  --corpus "$CORPUS_DIR/corpus.json" --targets "$CORPUS_DIR/targets.json" \
  --reports "$ORIGINAL_REPORTS" --json
```

All four paths are required. The checker validates original identities and
retained report bytes and reconstructs the replay verdict. It rejects missing,
unexpected, linked, or non-file bundle entries and does not change its inputs.
It does not launch an evaluator, server, package runner, health check, or state
adapter. `--json` prints the reconstructed artifact without echoing extension
metadata.

| Exit code | Meaning |
| --- | --- |
| `0` | Intact evidence with a passing replay verdict. |
| `1` | Intact evidence with a failed replay verdict. |
| `2` | Invalid or incomplete evidence; no JSON verdict is emitted. |

Consistency does not authenticate an author, prove that external checks ran,
or verify the recorded reason when no report bytes exist. Manage trusted
records of the original inputs and verification artifact separately. Hashes
alone do not protect against a bundle whose reports, hashes, and summaries were
rewritten together.

## Script options

| Script | Option | Meaning |
| --- | --- | --- |
| `collect.sh` | `--targets` | Target file. Required. |
| `collect.sh` | `--out` | New corpus path. Required. |
| `collect.sh` | `--binary` | Evaluator executable; default `target/release/mcpeval`. |
| `collect.sh` | `--min-observations` | Minimum observed targets for a zero exit; default `10`. |
| `collect.sh` | `--force` | Replace an existing corpus. |
| `collect.sh` | `--require-deployments` | Refuse targets without a prepared deployment. |
| `verify.mjs` | `--corpus` | Original corpus. Required. |
| `verify.mjs` | `--targets` | Original target file. Required. |
| `verify.mjs` | `--reports` | Original reports directory. Required. |
| `verify.mjs` | `--binary` | Evaluator executable; default `target/release/mcpeval`. |
| `verify.mjs` | `--out` | New evidence bundle directory. |
| `verify.mjs` | `--json` | Print the verification artifact. |
| `verify.mjs` | `--require-deployments` | Refuse targets without a prepared deployment. |
| `check-evidence.mjs` | `--evidence` | Evidence bundle directory. Required. |
| `check-evidence.mjs` | `--corpus` | Original corpus. Required. |
| `check-evidence.mjs` | `--targets` | Original target file. Required. |
| `check-evidence.mjs` | `--reports` | Original reports directory. Required. |
| `check-evidence.mjs` | `--json` | Print the reconstructed verification artifact. |
| `deployment.mjs` | `--root` | Absolute deployment directory to fingerprint. Required. |

`collect.sh` runs `collect.mjs` with the same options.

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

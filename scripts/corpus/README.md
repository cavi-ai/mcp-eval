# Corpus collection and verification

The checked-in standard/1 corpus is historical. It lacks package and evaluator
provenance and cannot be replayed as an exact collection. These tools refuse to
substitute today's packages or relabel old measurements.

New collection uses `mcpeval.readiness-corpus/v3`. Keep the target file and raw
reports with the corpus. The document contains evaluator version and executable
SHA-256, target-file and launch hashes, report hashes, package versions, call
counts, prerequisite names, and an outcome for every requested target.

Create a private target file with schema `mcpeval.corpus-targets/v1`, a
`standard` such as `mcpeval-standard/2`, and a `targets` array. Each target needs:

- `server`: a unique server label.
- `runtime`: `npm` or `uvx`.
- `package`, `version`, and `bin`: package name, an exact version, and its
  actual executable name. Versions cannot be tags, ranges, URLs, or wildcards.
- `args`: an explicit array, including `[]` when no arguments are needed.
- `prerequisites.environment`: required environment-variable names, including
  `[]` when none are needed. Values must be supplied at execution time.
- `prerequisites.checks`: named health-check commands, each with `name` and a
  `command` array. Checks must exit zero within ten seconds. Use `[]` only when
  the target needs no backing-service checks.
- Optional `prerequisites.state_checks`: named service-state assertions with
  `name`, an explicit `command` array, and an expected lowercase SHA-256 in
  `sha256`. Names must be unique across health and state checks. Omitting this
  field retains the existing health-only behavior.

State adapters must read the actual backing service and print only its stable
state digest, optionally followed by one newline, with a zero exit status.
They use the declared environment and fresh home, with a ten-second deadline
and a 1 KiB output limit. Store adapters in the prepared deployment to bind
their bytes too. Capture the expected digest when preparing the service
snapshot; the collector does not establish or update it automatically.

After health checks, collection and replay compare every state digest before
launching the evaluator and again after evaluation, even if evaluation failed.
A different digest is `errored` with `state-mismatch`. A failed, unavailable,
or malformed state check is `untested` before evaluation and `errored` after
it, with `state-check-failed`. These outcomes retain no observation or report.
Successful observations contain only state-check names and expected digests,
plus `state:` prerequisite references; adapter output is not persisted.
Original reports and the declared state identities must agree before replay.

Fingerprint the service identity and relevant dataset/configuration together.
An adapter's digest only proves equality of what it measures; an adapter that
prints a constant proves no service state. Keep credentials and sensitive
low-entropy values out of published digests. State checks do not restore,
freeze, or authenticate a service. Concurrent changes that are reverted between
checks remain undetectable. Use a read-only snapshot or service-supported
consistent read boundary and make the MCP server and adapter use that same
snapshot to mitigate this race. Changes outside the adapter's measured state
remain outside this evidence.

An optional `deployment` replaces the package runner with a prepared local
bundle. Include the installed dependencies and a native runtime executable in
one directory; prepare and validate it separately using the package ecosystem's
lock file. The collector does not install or resolve packages for this mode.

```sh
node scripts/corpus/deployment.mjs --root /absolute/private/deployment
```

Put the printed digest in the target's `deployment.sha256`, set
`deployment.root` to that absolute directory, and set `deployment.executable`
to the runtime's relative path, such as `bin/node`. Set the optional
`deployment.entrypoint` to the server's relative path, such as
`server/dist/index.js`. The command becomes the absolute executable path,
the absolute entrypoint path when present, and the target's existing `args`.
A native server can omit `entrypoint`. Use a native executable, not a script
that obtains an interpreter from the host. Package/version/bin fields remain
declared package identity; the operator must validate them against the bundle.

The versioned tree digest covers file names, bytes, permissions, directories,
and relative internal links. External, absolute, dangling, and cyclic links
and special files are refused. Collection and replay check the bundle before
health checks, after health checks, and after evaluation. A mismatch leaves the
target `errored` with `deployment-mismatch` and retains no observation or report.
Observations record `deployment_sha256`; absolute bundle paths stay in the
private target file. The existing launch hash binds those paths and arguments.
Targets without a bundle retain their existing top-level package pins.

Keep the bundle read-only during use, outside the fresh execution home, and
retain it with the target file and reports. Hash checks detect changes; they
do not prevent a concurrent writer from changing and restoring files between
checks. A read-only filesystem or immutable image mitigates that race. Runtime
libraries supplied by the operating system, dynamic downloads, commands found
through `PATH`, and imports outside the directory are outside this digest.
Use a deployment whose execution dependencies are contained in the directory;
the collector does not prove that dependency closure or sandbox its execution.

No target list or package version is inferred from historical observations.
The operator must declare all required services and credentials. Presence of
an environment variable alone does not prove that its credentials work; declare
a health check when validation is needed. Never put secrets in names or labels.
Arguments and environment values are not copied into the corpus. Hashes do not
redact low-entropy secrets, so do not publish private target files or treat a
launch hash as a credential protection mechanism.

```sh
cargo build --release --locked --bins
scripts/corpus/collect.sh --targets /absolute/private/targets.json \
  --out /absolute/private/corpus.json
node scripts/corpus/verify.mjs --corpus /absolute/private/corpus.json \
  --targets /absolute/private/targets.json \
  --reports /absolute/private/corpus.json.reports-UUID --json
```

Replay `--json` emits `mcpeval.corpus-verification/v1`. Redirect stdout to a
private file to retain it. Print its schema with
`mcpeval schema corpus-verification`. The artifact binds the exact corpus and
target-file bytes, evaluator version/executable digest, standard, platform, and
the ten-point reliability tolerance. It preserves the existing observation and
population counts and per-target results, adding `passed` and original/replay
report digests. Original unobserved targets have null scores and report digests;
missing or discarded replay reports have a null replay digest.

`passed` is true only when every target is observed and no area exceeds the
comparison tolerance. It means replay agreement, not server health: replay can
reproduce a consistently failing server. Equal raw-report digests are not a
pass requirement; timings and other measurements may differ within the policy.
Exit status is zero for a passing replay and one otherwise. Invalid input or
original-artifact provenance aborts before replay, emits an error on stderr,
and produces no verification artifact.

Keep the original corpus, targets, and reports with the verification artifact.
Without `--out`, the artifact identifies replay report bytes seen in memory but
does not retain them or include their payloads. To retain inspectable replay
evidence, select a new directory under an existing private parent:

```sh
node scripts/corpus/verify.mjs --corpus /absolute/private/corpus.json \
  --targets /absolute/private/targets.json \
  --reports /absolute/private/corpus.json.reports-UUID \
  --out /absolute/private/replay-evidence --json
```

The bundle contains `verification.json` and `reports/<server>.json` for each
replay report with a non-null digest. Report bytes match the artifact's replay
digests, including valid reports from unobserved outcomes. Missing, invalid,
or discarded reports are not saved. Original reports, targets, and the corpus
are not copied; retain them separately using the artifact's original digests.
`--out` works with text or JSON stdout and preserves the replay exit status.

The output directory is reserved exclusively before evaluator execution;
existing files, directories, or links are refused without replacement. There
is no force mode. On POSIX, new directories use owner-only permissions and
files use owner read/write permissions. On Windows, use a private parent with
appropriate ACLs. Raw reports can contain sensitive server output; the bundle
is not a redacted sharing envelope.

Reports are written before `verification.json`, whose complete bytes are
published atomically without overwriting a file. Completed failed verdicts
retain their bundle for inspection. Fatal errors remove the reserved incomplete
bundle. A process crash or failed cleanup can leave partial output; absence of
`verification.json` means the bundle is incomplete. Do not modify a reserved
directory during replay. This is not a transactional or crash-durable store.

Hashes and schema validation do not authenticate
the artifact's author. Labels are operator-supplied; review artifacts before
sharing them. A verification artifact does not prove the cause of score drift
or freeze host or service state.

`--binary` selects an explicit evaluator executable. Collection refuses an
existing output unless `--force` is passed. It writes the corpus atomically and
retains reports in the directory printed at completion. The default minimum
is ten observed targets; `--min-observations` explicitly changes it. A thin
candidate is retained for inspection and exits nonzero. It is not an approved
baseline. Publication of any new measurements remains a separate decision.

Missing environment or a failed health check leaves a target `untested` without
launching its server. No listed tools or no attempted tool calls also leaves a
target `untested`. Evaluation failures remain `errored`. Completed tool failures
are observed evidence, including when every attempted call failed; they are not
confused with tools that were never called. Only observations enter calibration.

Every target runs in a fresh home with only platform runtime variables and its
declared environment. Kubernetes uses the empty fixture configuration and
Docker uses an unreachable socket. This is environment isolation, not a security
sandbox. Evaluations have a 350-second process deadline and an 8 MiB output limit;
timeouts terminate the process tree. Prerequisite commands are also bounded.

Verification requires the original platform, evaluator executable, target file,
and matching report artifacts. It checks report projections before re-scoring.
Any untested, errored, or newly unavailable target prevents a verification pass.
Areas must match, with the existing ten-point reliability tolerance. Differences
report score drift; they do not establish its cause. Top-level package pins alone
do not lock transitive dependencies or runtime versions. Bundles bind the
prepared runtime and dependency bytes they contain. State checks bind the
declared service-state projection at the check boundaries. Credentials and the
host operating system remain external inputs; control them separately before
attributing drift to a server change. Artifact hashes bind bytes; they do not
authenticate authors.

Run deterministic fixture coverage with `npm run test:corpus`. This lane does
not download public servers or collect comparative measurements. Build both
release binaries first, or set `MCPEVAL_CORPUS_TEST_BINARY` to a built debug
evaluator; the native fixture uses the demo beside that executable.

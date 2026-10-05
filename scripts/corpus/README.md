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
report score drift; they do not establish its cause. Top-level package pins do
not lock transitive dependencies, runtime versions, service state, or credential
values. Use a separately locked deployment environment before attributing drift
to a server change. Artifact hashes bind bytes; they do not authenticate authors.

Run deterministic fixture coverage with `npm run test:corpus`. This lane does
not download public servers or collect comparative measurements. Build both
release binaries first, or set `MCPEVAL_CORPUS_TEST_BINARY` to a built debug
evaluator; the native fixture uses the demo beside that executable.

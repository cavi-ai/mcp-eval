# Continuous integration

The probe battery is built for CI gating: deterministic verdicts, fixed failure reasons, and a versioned JSON report with no timestamps, sessions, or payloads. The manifest's cases are the gate that sets the exit code; the readiness score comes from mcpeval's standard battery and never changes the exit code.

## GitHub Actions

A composite action installs mcpeval and runs the battery for downstream repositories. This repository runs it, as a downstream repository would, on pull requests and pushes to `main` that change the action, its scripts, the Rust sources, or the pinned distribution:

```yaml
name: mcp-eval
on: [push, pull_request]

jobs:
  probe:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: cavi-ai/mcp-eval@main
        with:
          server: my-server
          command: python3 scripts/launch-mcp-server.py --stdio
```

The action installs the release pinned by its own `distribution/release.json`:

1. It downloads the archive for the runner (`linux-x64`, `linux-arm64`,
   `darwin-x64`, `darwin-arm64`, `win32-x64`) and its `.sha256` companion.
2. It checks the companion's digest and file name, the pinned SHA-256, and the
   size. Any mismatch fails the step.
3. It adds `mcpeval` and `mcpeval-demo` to `PATH`.

`version: 0.2.0` installs that release instead, verified against that release's
own `release.json` rather than the action's. A `mcpeval` already on `PATH` is
used as is, so an earlier step can install a pinned or local build.

| Input | Default | Meaning |
| --- | --- | --- |
| `server` | required | Server label recorded in the report |
| `manifest` | `mcp-eval.manifest.json` | Probe manifest |
| `command` | | stdio server command, as words or a JSON array |
| `url` | | Streamable HTTP endpoint; set exactly one of `command` and `url` |
| `allow-mutation` | `false` | Authorize manifest-declared sandbox mutations |
| `allow-remote-http` | `false` | Allow a remote HTTPS `url` |
| `version` | | Release to install instead of the pinned one |
| `baseline` | | Committed report to gate against with `mcpeval diff --fail-on-regression` |
| `fail-on-change` | `false` | With `baseline`, also fail on changed failure reasons |
| `sarif` | `false` | Render SARIF and upload it to code scanning |
| `report-path` | `mcpeval.report.json` | Where the JSON report is written |

- `command` takes whitespace-separated words, with no quoting, or a JSON array
  of strings for arguments that contain spaces, such as
  `'["python3", "my server.py", "--stdio"]'`.
- For a Streamable HTTP server, pass `url: http://127.0.0.1:8080/mcp` instead of
  `command`; set exactly one of the two.
- Mutating manifests also require `allow-mutation: "true"`, and a remote HTTPS
  `url` requires `allow-remote-http: "true"`.
- Inputs reach the scripts through environment variables, never interpolated
  into shell code.

The report is rendered as markdown into the job summary and to `${report-path%.json}.md`, and written as JSON to `report-path`. The step exposes these outputs:

| Output | Meaning |
| --- | --- |
| `passed` | `true` when every selected case passed |
| `readiness` | Readiness score (0-100) under the mcpeval standard the report names |
| `report` | Path to the JSON report |
| `markdown` | Path to the rendered markdown report |
| `exit-code` | `mcpeval probe` exit code |
| `diff-exit-code` | `mcpeval diff` exit code, when `baseline` is set |
| `sarif` | Path to the SARIF document, when `sarif` is `true` |

- With `baseline: mcp-eval.baseline.json`, the action runs
  `mcpeval diff --fail-on-regression` against the committed report (plus
  `--fail-on-change` with `fail-on-change: "true"`) and appends its markdown
  table to the job summary and the `markdown` output.
- With `sarif: "true"`, it renders `mcpeval.sarif` located at the manifest's
  failing cases and uploads it through `github/codeql-action/upload-sarif`, also
  when the probe fails. The job needs `security-events: write`.
- The step exits with the probe's code, or with the diff's when the probe passed
  and the diff failed.

```yaml
jobs:
  probe:
    runs-on: ubuntu-latest
    permissions:
      contents: read
      security-events: write
    steps:
      - uses: actions/checkout@v4
      - uses: cavi-ai/mcp-eval@main
        with:
          server: my-server
          command: python3 scripts/launch-mcp-server.py --stdio
          baseline: mcp-eval.baseline.json
          sarif: 'true'
```

`baseline` and `sarif` need `mcpeval` 0.3.0 or later (`diff` and `report --manifest`); the action's pinned release has both. A `version` input older than 0.3.0 fails fast with a usage error naming the installed version.

## Saved corpus evidence

The separate corpus evidence action gates on an existing replay bundle using
the offline checker. Prepare the corpus, targets, original reports, and replay
bundle on the runner through your private storage policy, then pin the action
to a reviewed commit containing it:

```yaml
- name: Check corpus replay evidence
  id: corpus
  uses: cavi-ai/mcp-eval/actions/corpus-evidence@<reviewed-commit-sha>
  with:
    evidence: private/replay-evidence
    corpus: private/corpus.json
    targets: private/targets.json
    reports: private/original-reports
```

Replace the commit placeholder before use. With a local checkout of this
repository, use `./actions/corpus-evidence`. The action is separate from the
root probe action and is not in the npm package. It uses GitHub's built-in
`node24` runtime, which self-hosted runners must support, has no npm
dependencies, and does not install an evaluator or launch servers, package
runners, health checks, or state adapters.

| Input | Meaning |
| --- | --- |
| `evidence` | Replay bundle containing `verification.json` and `reports/`. |
| `corpus` | Original provenance-bearing corpus JSON. |
| `targets` | Original target file used for collection. |
| `reports` | Directory of original collection reports. |

All four inputs are required. Relative paths resolve from the calling
workspace; absolute paths also work.

| Output | Meaning |
| --- | --- |
| `evidence-valid` | `true` when offline consistency checks pass. |
| `passed` | `true` only for intact passing replay evidence. |
| `exit-code` | `0` for intact passing evidence, `1` for intact failed evidence, `2` for invalid/incomplete evidence or reporting failure. |
| `observations` | Original observed-target count; empty for invalid evidence. |
| `population` | Original target-population count; empty for invalid evidence. |

Failed or invalid evidence fails the step. Outputs and the job summary are
written before the verdict exit when runner file commands are available.
Reporting errors also fail the step and can leave outputs unavailable. If using
`continue-on-error` to inspect outputs, keep an explicit downstream failure gate
and treat missing outputs as failure. A pass means replay agreement, not server
health or author authentication.

The action's diagnostics and summary contain fixed messages, counts, and
verdicts. They omit server labels, paths, report payloads, digests, and
artifact metadata, and the action uploads no reports. GitHub can log supplied
inputs independently, so use secrets for sensitive paths and private storage
with appropriate access controls. Do not upload raw evidence to public workflow
artifacts. Follow
[Corpus collection and replay](corpus-replay.md) to prepare and retain the files.

## Committed baselines

The JSON report is deterministic, so it can be committed and diffed:

```sh
mcpeval probe --server demo --format json -- your-mcp-server --flags > mcp-eval.baseline.json
```

Gate the current run against the baseline with `mcpeval diff`:

```yaml
      - name: Probe battery
        run: mcpeval probe --server demo --format json -- your-mcp-server --flags > current.json

      - name: Baseline diff
        run: mcpeval diff mcp-eval.baseline.json current.json --fail-on-regression
```

The diff matches cases by id and probe kind and classifies each as regressed,
fixed, changed (still failing, for a different reason), or unchanged. Both
documents must name the same server.

- It prints the readiness movement, or `not comparable` when the documents were
  scored under different standards or the baseline is a v1 document.
- `--fail-on-regression` exits non-zero only for regressions. Fixes and added
  cases are informational, since manifest growth is deliberate.
- `--fail-on-change` also gates on changed failure reasons.
- Without `--fail-on-regression` the diff is informational and always exits
  zero.
- `--format json` emits a deterministic `mcpeval.probe-diff/v2` document;
  `--format markdown` renders a pull-request-ready table.

```sh
mcpeval diff baseline.json report.json
# slow_read-latency           REGRESSED latency-budget-exceeded  411 ms → 2016 ms (+1605)
# readiness  91 → 91
# 1 regressed, 0 fixed, 0 changed, 24 unchanged, 0 removed, 0 added
```

Both inputs are share-safe reports, so every diff format carries only case IDs,
verdicts, fixed reason labels, and measurement numbers.

Regenerate the baseline deliberately and review the diff in a pull request — never regenerate it inside CI, or the gate gates nothing.

## Verifying findings in CI

After a fix lands, run the finding's probe case in CI. Three consecutive green runs close the finding; any red reopens it:

```sh
mcpeval verify --finding finding-0123456789abcdef \
  --case literal-status --manifest mcp-eval.manifest.json \
  -- your-mcp-server --flags
```

## Readiness badge

The JSON report's `readiness.badge` is a static shields.io URL that encodes only
the score and its color band. To keep a README badge current, write it into the
README on a schedule:

```sh
BADGE=$(mcpeval probe --server demo --format json -- your-mcp-server --flags | jq -r '.readiness.badge // empty')
[ -n "$BADGE" ] && sed -i "s|https://img.shields.io/badge/mcpeval-[^)]*|$BADGE|" README.md
```

No payload or server detail leaves the repository.

## Other CI systems

```sh
cargo install mcpeval --locked
mcpeval probe --server demo --manifest mcp-eval.manifest.json -- your-mcp-server --flags
```

Any runner with a Rust toolchain works: the battery is a single static binary
with no network services. `mcpeval probe` exits non-zero when any selected case
fails, so it drops into `make check`, pre-merge hooks, or any pipeline
unchanged.

| Exit | Meaning |
| --- | --- |
| `0` | Every selected case passed |
| `1` | A red verdict |
| `2` | A usage error |
| `3` | The run could not complete; the report is still written |

A pipeline can retry exit 3 without retrying red servers.

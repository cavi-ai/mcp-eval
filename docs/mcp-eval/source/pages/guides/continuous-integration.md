# Continuous integration

The probe battery is built for CI gating: deterministic verdicts, fixed failure reasons, and a versioned JSON report with no timestamps, sessions, or payloads. The manifest's cases are the gate that sets the exit code; the readiness score comes from mcpeval's standard battery and never changes the exit code.

## GitHub Actions

A composite action installs mcpeval and runs the battery for downstream repositories:

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

The action installs the release pinned by its own `distribution/release.json`: it downloads the archive for the runner (`linux-x64`, `linux-arm64`, `darwin-x64`, `darwin-arm64`, `win32-x64`) and its `.sha256` companion, checks the companion's digest and file name, the pinned SHA-256, and the size, and adds `mcpeval` and `mcpeval-demo` to `PATH`. Any mismatch fails the step. `version: 0.2.0` installs that release instead, verified against that release's own `release.json` rather than the action's. A `mcpeval` already on `PATH` is used as is.

`command` takes whitespace-separated words, with no quoting, or a JSON array of strings for arguments that contain spaces, such as `'["python3", "my server.py", "--stdio"]'`. For a Streamable HTTP server, pass `url: http://127.0.0.1:8080/mcp` instead of `command`; set exactly one of the two. Mutating manifests additionally require `allow-mutation: "true"`, and a remote HTTPS `url` requires `allow-remote-http: "true"`. Inputs reach the scripts through environment variables, never interpolated into shell code.

The report is rendered as markdown into the job summary and written as JSON to `report-path` (default `mcpeval.report.json`). The step exposes these outputs:

| Output | Meaning |
| --- | --- |
| `passed` | `true` when every selected case passed |
| `readiness` | Readiness score (0-100) under the mcpeval standard the report names |
| `report` | Path to the JSON report |
| `markdown` | Path to the rendered markdown report |
| `exit-code` | `mcpeval probe` exit code |
| `diff-exit-code` | `mcpeval diff` exit code, when `baseline` is set |
| `sarif` | Path to the SARIF document, when `sarif` is `true` |

With `baseline: mcp-eval.baseline.json`, the action runs `mcpeval diff --fail-on-regression` against the committed report (plus `--fail-on-change` with `fail-on-change: "true"`) and appends its markdown table to the job summary and the `markdown` output. With `sarif: "true"`, it renders `mcpeval.sarif` located at the manifest's failing cases and uploads it through `github/codeql-action/upload-sarif`, also when the probe fails; the job needs `security-events: write`. The step exits with the probe's code, or with the diff's when the probe passed and the diff failed.

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

Replace the commit placeholder before use. Relative input paths resolve from
the calling workspace; all four inputs are required. With a local checkout of
this repository, use `./actions/corpus-evidence`. The action uses GitHub's Node
24 runtime and does not install an evaluator or launch servers, package runners,
health checks, or state adapters.

| Output | Meaning |
| --- | --- |
| `evidence-valid` | `true` when offline consistency checks pass. |
| `passed` | `true` only for intact passing replay evidence. |
| `exit-code` | `0` for intact passing evidence, `1` for intact failed evidence, `2` for invalid/incomplete evidence or reporting failure. |
| `observations` | Original observed-target count; empty for invalid evidence. |
| `population` | Original target-population count; empty for invalid evidence. |

Failed or invalid evidence fails the step. Reporting errors also fail it and
can leave outputs unavailable. If using `continue-on-error` to inspect outputs,
keep an explicit downstream failure gate and treat missing outputs as failure.
A pass means replay agreement, not server health or author authentication.

The action's diagnostics and summary contain fixed messages, counts, and
verdicts; it does not upload reports. GitHub can log supplied inputs independently,
so use secrets for sensitive paths and private storage with appropriate access
controls. Do not upload raw evidence to public workflow artifacts. Follow
[Corpus collection and replay](corpus-replay.md) to prepare and retain the files.

## Committed baselines

The JSON report is deterministic, so it can be committed and diffed:

```sh
mcpeval probe --server demo --format json > mcp-eval.baseline.json
```

Gate the current run against the baseline with `mcpeval diff`:

```yaml
      - name: Probe battery
        run: mcpeval probe --server demo --format json > current.json

      - name: Baseline diff
        run: mcpeval diff mcp-eval.baseline.json current.json --fail-on-regression
```

The diff classifies every case as regressed, fixed, changed (still failing, for a different reason), or unchanged (matched by case id and probe kind), prints the readiness movement (or `not comparable` when the two documents were scored under different standards, or the baseline is a v1 document), and exits non-zero only for regressions — fixes and added cases are informational, since manifest growth is deliberate. Add `--fail-on-change` to also gate on changed failure reasons. Both documents must name the same server. `--format json` emits a deterministic `mcpeval.probe-diff/v1` document; `--format markdown` renders a pull-request-ready table. Without `--fail-on-regression` the diff is informational and always exits zero.

Regenerate the baseline deliberately and review the diff in a pull request — never regenerate it inside CI, or the gate gates nothing.

## Verifying findings in CI

After a fix lands, run the finding's probe case in CI. Three consecutive green runs close the finding; any red reopens it:

```sh
mcpeval verify --finding finding-0123456789abcdef \
  --case literal-status --manifest mcp-eval.manifest.json \
  -- your-mcp-server --flags
```

## Other CI systems

Any runner with a Rust toolchain works — the battery is a single static binary with no network services. `mcpeval probe` exits non-zero when any selected case fails, so it drops into `make check`, pre-merge hooks, or any pipeline unchanged. Exit 1 is a red verdict, 2 a usage error, and 3 a run that could not complete (the report is still written), so a pipeline can retry infrastructure failures without retrying red servers.

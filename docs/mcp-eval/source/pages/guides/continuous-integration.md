# Continuous integration

The probe battery is built for CI gating: deterministic verdicts, fixed failure reasons, and a versioned JSON report with no timestamps, sessions, or payloads.

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
| `readiness` | Readiness score |
| `report` | Path to the JSON report |
| `markdown` | Path to the rendered markdown report |
| `exit-code` | `mcpeval probe` exit code |
| `diff-exit-code` | `mcpeval diff` exit code, when `baseline` is set |
| `sarif` | Path to the SARIF document, when `sarif` is `true` |

With `baseline: mcp-eval.baseline.json`, the action runs `mcpeval diff --fail-on-regression` against the committed report (plus `--fail-on-change` with `fail-on-change: "true"`) and appends its markdown table to the job summary and the `markdown` output. With `sarif: "true"`, it renders `mcpeval.sarif` located at the manifest's failing cases and uploads it through `github/codeql-action/upload-sarif`, also when the probe fails; the job needs `security-events: write`. The step exits with the probe's code, or with the diff's when the probe passed and the diff failed.

`baseline` and `sarif` need `mcpeval` 0.3.0 or later (`diff` and `report --manifest`). The pinned release moves when `distribution/release.json` is updated at release time; until then, set `version: '0.3.0'` or later, or the step fails fast with a usage error naming the installed version.

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

The diff classifies every case as regressed, fixed, changed (still failing, for a different reason), or unchanged (matched by case id and probe kind), prints the readiness movement, and exits non-zero only for regressions — fixes and added cases are informational, since manifest growth is deliberate. Add `--fail-on-change` to also gate on changed failure reasons. Both documents must name the same server. `--format json` emits a deterministic `mcpeval.probe-diff/v1` document; `--format markdown` renders a pull-request-ready table. Without `--fail-on-regression` the diff is informational and always exits zero.

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
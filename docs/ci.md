# Continuous integration

The probe battery is designed for CI gating: deterministic verdicts, fixed
failure reasons, and a versioned JSON report with no timestamps, sessions,
or payloads. This guide covers the copy-paste recipes; the composite action
at [action.yml](../action.yml) wraps them.

## GitHub Actions

### stdio server

```yaml
name: mcp-eval
on: [push, pull_request]

jobs:
  probe:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - id: mcpeval
        uses: cavi-ai/mcp-eval@main
        with:
          server: my-server
          command: python3 scripts/launch-mcp-server.py --stdio
```

The action installs the release pinned by its own
`distribution/release.json`: it downloads the archive for the runner
(`linux-x64`, `linux-arm64`, `darwin-x64`, `darwin-arm64`, `win32-x64`) and
its `.sha256` companion, checks the companion's digest and file name, the
pinned SHA-256, and the size, and adds `mcpeval` and `mcpeval-demo` to
`PATH`. Any mismatch fails the step. `version: 0.2.0` installs that release
instead, verified against that release's own `release.json` rather than the
action's. A `mcpeval` already on `PATH` is used as is, so an earlier step can
install a pinned or local build.

`command` takes either whitespace-separated words, with no quoting, or a
JSON array of strings for arguments that contain spaces:

```yaml
          command: '["python3", "my server.py", "--stdio"]'
```

Inputs are passed to the scripts through environment variables, never
interpolated into shell code.

| Input | Default | Meaning |
| --- | --- | --- |
| `server` | required | Server label recorded in the report |
| `manifest` | `mcp-eval.manifest.json` | Probe manifest |
| `command` | | stdio server command (words or JSON array) |
| `url` | | Streamable HTTP endpoint; set exactly one of `command` and `url` |
| `allow-mutation` | `false` | Authorize manifest-declared sandbox mutations |
| `allow-remote-http` | `false` | Allow a remote HTTPS `url` |
| `version` | | Release to install instead of the pinned one |
| `baseline` | | Committed report to gate against with `mcpeval diff --fail-on-regression` |
| `fail-on-change` | `false` | With `baseline`, also fail on changed failure reasons |
| `sarif` | `false` | Render SARIF and upload it to code scanning |
| `report-path` | `mcpeval.report.json` | Where the JSON report is written |

| Output | Meaning |
| --- | --- |
| `passed` | `true` when every selected case passed |
| `readiness` | Readiness score |
| `report` | Path to the JSON report |
| `markdown` | Path to the rendered markdown report |
| `exit-code` | `mcpeval probe` exit code |
| `diff-exit-code` | `mcpeval diff` exit code, when `baseline` is set |
| `sarif` | Path to the SARIF document, when `sarif` is `true` |

The report is rendered as markdown into the job summary and to the
`markdown` output path (`${report-path%.json}.md`, including the baseline
diff when `baseline` is set). The step exits with the probe's code, or
with the diff's when the probe passed and the diff failed.

`baseline` and `sarif` need `mcpeval` 0.3.0 or later (`diff` and `report
--manifest`). The pinned release moves when `distribution/release.json`
is updated at release time; until then, set `version: '0.3.0'` or later,
or the step fails fast with a usage error naming the installed version.

### Streamable HTTP server

Start your server in a background step, then probe its endpoint:

```yaml
      - uses: cavi-ai/mcp-eval@main
        with:
          server: my-server
          url: http://127.0.0.1:8080/mcp
```

### Baseline gate in the action

```yaml
      - uses: cavi-ai/mcp-eval@main
        with:
          server: my-server
          command: python3 scripts/launch-mcp-server.py --stdio
          baseline: mcp-eval.baseline.json
```

The diff's markdown table is appended to the job summary and its exit code
is the `diff-exit-code` output.

### SARIF to code scanning

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
          sarif: 'true'
```

The action renders `mcpeval.sarif` with results located at the manifest's
failing cases and uploads it with `github/codeql-action/upload-sarif`, also
when the probe fails.

### Committed baseline diff

The JSON report is deterministic (no timestamps, sessions, or payloads), so
it can be committed and diffed:

```sh
mcpeval probe --server demo --format json > mcp-eval.baseline.json
```

A workflow that fails when the battery regresses relative to the baseline:

```yaml
      - name: Probe battery
        run: mcpeval probe --server demo --format json > current.json

      - name: Baseline diff
        run: mcpeval diff mcp-eval.baseline.json current.json --fail-on-regression
```

The diff classifies every case as regressed, fixed, changed (still
failing, for a different reason), or unchanged (matched by case id and
probe kind), prints the readiness movement, and exits non-zero only for
regressions — fixes and added cases are informational, since manifest
growth is deliberate. Add `--fail-on-change` to also gate on changed
failure reasons. Both documents must name the same server. `--format json` emits a deterministic
`mcpeval.probe-diff/v1` document, and `--format markdown` renders a
pull-request-ready table. Without `--fail-on-regression` the diff is
informational and always exits zero.

Regenerate the baseline deliberately and review the diff in the PR — never
regenerate it inside CI, or the gate gates nothing.

## Readiness badge

`mcpeval probe --format markdown` prints a ready-to-paste report section
with a readiness score and a static shields.io badge URL. To keep a live
badge in your README, commit the score into the URL on a schedule:

```sh
SCORE=$(mcpeval probe --server demo --format json | jq '.readiness.score')
sed -i "s|mcpeval-[0-9]*%2F100|mcpeval-${SCORE}%2F100|" README.md
```

Because the badge URL encodes only the score, no payload or server detail
ever leaves your repository.

## Other CI systems

Any runner with a Rust toolchain works — the battery is a single static
binary with no network services:

```sh
cargo install mcpeval --locked
mcpeval probe --server demo --manifest mcp-eval.manifest.json -- your-mcp-server --flags
```

The command exits non-zero when any selected case fails, so it drops into
`make check`, pre-merge hooks, or any pipeline unchanged. Exit 1 is a red
verdict, 2 a usage error, and 3 a run that could not complete (the report
is still written), so a pipeline can retry infrastructure failures without
retrying red servers.

## Verifying findings in CI

After a fix lands, run the finding's probe case in CI; three consecutive
green runs close the finding, and any red reopens it:

```sh
mcpeval verify --finding finding-0123456789abcdef \
  --case literal-status --manifest mcp-eval.manifest.json \
  -- your-mcp-server --flags
```
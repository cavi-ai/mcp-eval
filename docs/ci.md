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
      - uses: dtolnay/rust-toolchain@stable
      - uses: cavi-ai/mcp-eval@main
        with:
          server: my-server
          command: python3 scripts/launch-mcp-server.py --stdio
```

`cargo install mcpeval` is skipped when `mcpeval` is already on PATH, so you
can pre-install a pinned build in an earlier step to keep runs fast.

### Streamable HTTP server

Start your server in a background step, then probe its endpoint:

```yaml
      - uses: cavi-ai/mcp-eval@main
        with:
          server: my-server
          url: http://127.0.0.1:8080/mcp
```

### Committed baseline diff

The JSON report is deterministic (no timestamps, sessions, or payloads), so
it can be committed and diffed:

```sh
mcpeval probe --server demo --format json > mcp-eval.baseline.json
```

A workflow that fails when the battery regresses relative to the baseline:

```yaml
      - name: Baseline diff
        run: |
          mcpeval probe --server demo --format json > current.json
          diff <(jq -S . mcp-eval.baseline.json) <(jq -S . report.json) || {
            echo "::error::probe results drifted from the committed baseline"
            exit 1
          }
```

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
`make check`, pre-merge hooks, or any pipeline unchanged.

## Verifying findings in CI

After a fix lands, run the finding's probe case in CI; three consecutive
green runs close the finding, and any red reopens it:

```sh
mcpeval verify --finding finding-0123456789abcdef \
  --case literal-status --manifest mcp-eval.manifest.json \
  -- your-mcp-server --flags
```
# Continuous integration

The probe battery is built for CI gating: deterministic verdicts, fixed failure reasons, and a versioned JSON report with no timestamps, sessions, or payloads.

## GitHub Actions

A composite action wraps installation and the battery for downstream repositories:

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

`cargo install mcpeval` is skipped when `mcpeval` is already on `PATH`. For a Streamable HTTP server, pass `url: http://127.0.0.1:8080/mcp` instead of `command`. Mutating manifests additionally require `allow-mutation: "true"`.

## Committed baselines

The JSON report is deterministic, so it can be committed and diffed:

```sh
mcpeval probe --server demo --format json > mcp-eval.baseline.json
```

Regenerate the baseline deliberately and review the diff in a pull request — never regenerate it inside CI, or the gate gates nothing.

## Verifying findings in CI

After a fix lands, run the finding's probe case in CI. Three consecutive green runs close the finding; any red reopens it:

```sh
mcpeval verify --finding finding-0123456789abcdef \
  --case literal-status --manifest mcp-eval.manifest.json \
  -- your-mcp-server --flags
```

## Other CI systems

Any runner with a Rust toolchain works — the battery is a single static binary with no network services. `mcpeval probe` exits non-zero when any selected case fails, so it drops into `make check`, pre-merge hooks, or any pipeline unchanged.
# Contributing to mcp-eval

Thank you for contributing. Keep changes focused, privacy-preserving, and covered
by tests.

## Development setup

Install the Rust toolchain declared in `rust-toolchain.toml`, then build and test:

```sh
cargo build
cargo test --all-targets --locked -- --test-threads=1
```

## Pull requests

- Base work on the current `main` branch.
- Add regression tests before changing behavior.
- Keep commits focused and use factual commit and pull-request descriptions.
- Update user-facing documentation and `CHANGELOG.md` when behavior changes.
- Do not commit local plans, transcripts, captured MCP payloads, credentials,
  machine-specific paths, or other private artifacts.
- Use synthetic fixtures. Privacy canaries must be obviously fictitious and must
  be asserted absent from persisted output.

Run the checks in [`.pr-gates`](.pr-gates) before opening a pull request.
GitHub runs one Ubuntu job for each pull request and push to `main`. The
action and corpus tests use the binaries Cargo built for the Rust test suite.

For platform-specific changes, dispatch **Native checks** and choose the
affected runner. It runs the native suite and optimized timing tests, then
exercises the npm and composite action installers. Run the optimized suite
locally before releasing:

```sh
cargo test --release --locked -- --test-threads=1
```

Native tests run serially to prevent fixture workloads from overlapping timing
measurements. The release lane also exercises the large-frame budget, which is
not compiled in debug builds. Both timing limits remain 2 ms; output records
baseline and shim timings, with sample counts and maxima for the p95 check.
Run these lanes on a quiet machine. Serialization controls this test suite,
not unrelated host processes. Investigate a failure using the recorded
measurements; a passing retry alone does not establish its cause.

## Privacy boundary

Changes must preserve the read-only default and the documented persistence
boundary. Raw request values, response bodies, error prose, salts, and sandbox
details must not enter shareable output. Mutating probes require both a declared
sandbox and explicit operator opt-in.

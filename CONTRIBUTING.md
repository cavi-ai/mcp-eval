# Contributing to mcp-eval

Thank you for contributing. Keep changes focused, privacy-preserving, and covered
by tests.

## Development setup

Install the Rust toolchain declared in `rust-toolchain.toml`, then build and test:

```sh
cargo build
cargo test --all-targets
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

Before opening a pull request, run:

```sh
cargo fmt -- --check
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
cargo test --release
git diff --check
```

## Privacy boundary

Changes must preserve the read-only default and the documented persistence
boundary. Raw request values, response bodies, error prose, salts, and sandbox
details must not enter shareable output. Mutating probes require both a declared
sandbox and explicit operator opt-in.

# Release runbook

How to cut a release. The pipelines do the rest; the human steps are tag,
secrets, and post-release announcements.

## What ships on tag push

Pushing a tag `vX.Y.Z` triggers two independent pipelines:

| Pipeline | Trigger | Produces |
| --- | --- | --- |
| [release-binaries.yml](../.github/workflows/release-binaries.yml) | `push: tags v*.*.*` | Prebuilt binaries for `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `x86_64-apple-darwin`, `aarch64-apple-darwin`, `x86_64-pc-windows-msvc`, each with a SHA256 checksum, attached to the GitHub release; crates.io publish |
| [publish-docs.yml](../.github/workflows/publish-docs.yml) | `release: published` | Deterministic versioned docs archive `mcp-eval-docs-vX.Y.Z.tar.gz` + release envelope dispatched to cavi-home |

Binary uploads and the crates.io publish are idempotent: re-running against
an existing release or published version verifies identical bytes and exits
cleanly instead of overwriting.

## Human steps

1. **Freeze and bump.** Set `version` in `Cargo.toml` to the release version,
   update `CHANGELOG.md` (move the Unreleased section under the new version),
   and run the full gates: `cargo fmt --check && cargo clippy --all-targets
   -- -D warnings && cargo test --all-targets`. Commit on main via PR.
2. **Tag.** `git tag vX.Y.Z && git push origin vX.Y.Z` — the tag must point
   at the exact release commit; both pipelines verify this.
3. **Create the GitHub release** for the tag (`gh release create vX.Y.Z
   --title "mcp-eval vX.Y.Z" --generate-notes`) — this triggers the docs
   pipeline; the binaries pipeline attaches assets to it as they build.
4. **Secrets** (one-time, repository settings):
   - `CARGO_REGISTRY_TOKEN` — crates.io token with publish rights for
     `mcpeval`. Without it the binaries still attach but the crates.io step
     fails; publish manually with `cargo publish --locked` and re-run the
     workflow if preferred.
5. **Homebrew.** With no tap yet, install stays `cargo install mcpeval
   --locked`. When a `cavi-ai/homebrew-tap` exists, add a formula pinning
   the two macOS archive SHA256 sums from the release assets, then update
   the README install section to `brew install cavi-ai/tap/mcpeval`.
6. **Announce.** Update the README badge/badges section if the readiness
   score changed, and post release notes links wherever the project is
   discussed.

## Verification after release

```sh
# install from crates.io
cargo install mcpeval --locked

# or download a prebuilt binary and verify its checksum
curl -fsSLO https://github.com/cavi-ai/mcp-eval/releases/download/vX.Y.Z/mcpeval-aarch64-apple-darwin.tar.gz
curl -fsSLO https://github.com/cavi-ai/mcp-eval/releases/download/vX.Y.Z/mcpeval-aarch64-apple-darwin.tar.gz.sha256
shasum -a 256 -c mcpeval-aarch64-apple-darwin.tar.gz.sha256

mcpeval --version
```

## Pre-1.0 policy

Before 1.0 the CLI, on-disk schema, and manifest format may change between
minor versions. The changelog is the contract; manifests are pinned to
`"version": 1` and validated strictly, so a manifest that validated with one
release keeps validating with any release until the format version is
deliberately bumped.
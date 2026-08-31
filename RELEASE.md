# Release runbook

How to cut a release. The pipelines do the rest; the human steps are tag,
secrets, and post-release announcements.

## What ships on tag push

Pushing a tag `vX.Y.Z` triggers two independent pipelines:

| Pipeline | Trigger | Produces |
| --- | --- | --- |
| [release-binaries.yml](../.github/workflows/release-binaries.yml) | `push: tags v*.*.*` | Prebuilt binaries for `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `x86_64-apple-darwin`, `aarch64-apple-darwin`, `x86_64-pc-windows-msvc`, each with a SHA256 checksum, attached to the GitHub release; crates.io publish |
| [publish-docs.yml](../.github/workflows/publish-docs.yml) | `release: published` | Deterministic versioned docs archive `mcp-eval-docs-vX.Y.Z.tar.gz` + release envelope dispatched to cavi-home |
| [publish-distributions.yml](../.github/workflows/publish-distributions.yml) | manual, exact release confirmation | Checksummed npm package publication and/or a pull request against `cavi-ai/homebrew-tap` |

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
   - Create a protected `npm` environment with a one-time
     `NPM_BOOTSTRAP_TOKEN`. npm requires the package to exist before trusted
     publishing can be configured, so this credential bootstraps v0.1.0 with
     provenance. After publication, delete it and configure trusted publishing
     for `@cavi-ai/mcp-eval`, repository `cavi-ai/mcp-eval`, workflow
     `publish-distributions.yml`, and environment `npm`.
   - `HOMEBREW_TAP_TOKEN` — a fine-grained token that can push a branch and
     open a pull request in `cavi-ai/homebrew-tap`; protect it with the
     `homebrew` environment.
5. **Verify distribution inputs.** Run `npm run test:distribution`, then
   `node scripts/distribution/verify.mjs --online`. The online gate downloads
   every archive and companion checksum named by `distribution/release.json`
   and refuses any byte mismatch.
6. **Publish npm/Homebrew.** Dispatch `publish-distributions.yml`, select the
   intended target, and enter the exact confirmation shown by the workflow.
   The initial npm publish uses the protected bootstrap credential and
   provenance; later releases use trusted publishing. Homebrew publication
   stages `Formula/mcpeval.rb` plus the tap index and opens a reviewable pull
   request; it never pushes directly to the tap's default branch.
7. **Announce.** Update the README badge/badges section if the readiness
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

After the corresponding registry and tap publications are visible, verify the
additional install paths:

```sh
npm install --global @cavi-ai/mcp-eval@0.1.0
mcpeval --version

brew install cavi-ai/tap/mcpeval
brew test cavi-ai/tap/mcpeval
```

The v0.1.0 npm package and formula were added after the tag was created. Their
source therefore lives on the post-release branch, while
`distribution/release.json` pins the original tag commit, filenames, and
published archive digests. The manual workflow exists specifically to publish
those definitions without rebuilding or replacing release assets.

## Pre-1.0 policy

Before 1.0 the CLI, on-disk schema, and manifest format may change between
minor versions. The changelog is the contract; manifests are pinned to
`"version": 1` and validated strictly, so a manifest that validated with one
release keeps validating with any release until the format version is
deliberately bumped.

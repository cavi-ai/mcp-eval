# Release runbook

One tag ships everything. The pipelines create the GitHub Release, attach the
binaries and the distribution manifest, publish the crate and the npm package,
and publish the documentation. The human steps are the version bump, the tag,
the post-release refresh of the committed distribution files, and the Homebrew
tap update.

## What ships on tag push

| Pipeline | Trigger | Produces |
| --- | --- | --- |
| [ci.yml](.github/workflows/ci.yml) | push to `main`, pull requests, manual | fmt, clippy, build, tests on Linux/macOS/Windows, version agreement, docs artifact, distribution contract, npm install and Homebrew install end-to-end |
| [release-binaries.yml](.github/workflows/release-binaries.yml) | `push: tags v*`; manual with `tag` to backfill | `mcpeval` + `mcpeval-demo` for `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `x86_64-apple-darwin`, `aarch64-apple-darwin`, `x86_64-pc-windows-msvc`, each with a `.sha256` companion; the GitHub Release itself, with notes from the CHANGELOG section; `release.json` and `mcpeval.rb` generated from the built bytes; then calls `publish-docs` |
| [publish-docs.yml](.github/workflows/publish-docs.yml) | called by `release-binaries`; `release: published`; manual (`dry_run` default `true`) | `mcp-eval-docs-vX.Y.Z.tar.gz` + `.sha256` on the release, and the release envelope dispatched to cavi-home |
| [publish-crates.yml](.github/workflows/publish-crates.yml) | `push: tags v*`; manual stops at `--dry-run` | `mcpeval` on crates.io |
| [publish.yml](.github/workflows/publish.yml) | `push: tags v*`; manual publishes the `package.json` version if it is not on npm yet | `@cavi-ai/mcp-eval` on npm through trusted publishing (OIDC) with provenance; waits for `release.json` on the release, verifies every pinned archive byte-for-byte, then publishes |

Asset uploads are idempotent and never overwrite: an asset that already exists
must be byte-identical or the run fails. The npm package and the Homebrew
formula pin the archives by size and SHA-256 through `distribution/release.json`.

## Human steps

1. **Bump.** Set `version` in `Cargo.toml` and `package.json` to the release
   version, run `cargo update --workspace` so the `mcpeval` entry in
   `Cargo.lock` follows (every `--locked` gate refuses a stale lockfile), and
   move the `## Unreleased` CHANGELOG entries under `## X.Y.Z - YYYY-MM-DD`.
   Run `python3 scripts/check-version-agreement.py` and the gates in
   [`.pr-gates`](.pr-gates). Merge on `main` via pull request. The committed
   `distribution/release.json` and `Formula/mcpeval.rb` still describe the
   previous release at this point; CI accepts that lag.
2. **Tag.** `git tag vX.Y.Z && git push origin vX.Y.Z` on the merge commit.
   `release-binaries`, `publish-crates`, and `publish` start from the tag;
   `publish-docs` runs when the release exists.
3. **Watch.** The release carries 10 binary assets, `release.json`,
   `mcpeval.rb`, and the docs archive with its `.sha256`. `npm view
   @cavi-ai/mcp-eval@X.Y.Z` and `cargo info mcpeval@X.Y.Z` resolve.
4. **Refresh the committed distribution.** Download `release.json` from the
   release into `distribution/release.json`, run
   `node scripts/distribution/render-formula.mjs > Formula/mcpeval.rb`, run
   `npm run test:distribution && npm run verify:distribution`, and merge via
   pull request.
5. **Homebrew tap.** In a checkout of `cavi-ai/homebrew-tap`, run
   `node scripts/distribution/stage-homebrew.mjs --source <this repo> --tap
   <tap checkout>`, then commit `mcpeval X.Y.Z` and open the tap pull request.
   `brew install cavi-ai/tap/mcpeval` resolves once it merges.
6. **Announce.** Update the README badges if the readiness score changed.

## One-time configuration

- Repository secrets: `CARGO_REGISTRY_TOKEN` (crates.io publish rights for
  `mcpeval`), `CONSUMER_DISPATCH_TOKEN` (repository dispatch to
  `cavi-ai/cavi-home`).
- Environment `production`: no secrets. It is named in the npm trusted
  publisher and is part of the OIDC subject claim.
- npm trusted publisher for `@cavi-ai/mcp-eval` on npmjs.com: organization
  `cavi-ai`, repository `mcp-eval`, workflow filename `publish.yml`,
  environment `production`, with `npm publish` allowed (configurations created
  after 2026-09-03 default to `npm stage publish` only). Under *Publishing
  access*, require two-factor authentication and disallow tokens.
- No npm token is stored anywhere; `scripts/distribution/publish-auth.test.mjs`
  fails if one returns to the workflow.

## Verification after release

```sh
cargo install mcpeval --locked
mcpeval --version

npm install --global @cavi-ai/mcp-eval@X.Y.Z
mcpeval --version

curl -fsSLO https://github.com/cavi-ai/mcp-eval/releases/download/vX.Y.Z/mcpeval-aarch64-apple-darwin.tar.gz
curl -fsSLO https://github.com/cavi-ai/mcp-eval/releases/download/vX.Y.Z/mcpeval-aarch64-apple-darwin.tar.gz.sha256
shasum -a 256 -c mcpeval-aarch64-apple-darwin.tar.gz.sha256

brew install cavi-ai/tap/mcpeval
brew test cavi-ai/tap/mcpeval
```

## Pre-1.0 policy

Before 1.0 the CLI, on-disk schema, and manifest format may change between
minor versions. The changelog is the contract; manifests are pinned to
`"version": 1` and validated strictly, so a manifest that validated with one
release keeps validating with any release until the format version is
deliberately bumped.

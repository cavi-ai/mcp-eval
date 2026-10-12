# mcp-eval

[![CI](https://github.com/cavi-ai/mcp-eval/actions/workflows/ci.yml/badge.svg)](https://github.com/cavi-ai/mcp-eval/actions/workflows/ci.yml)
[![npm](https://img.shields.io/npm/v/@cavi-ai/mcp-eval)](https://www.npmjs.com/package/@cavi-ai/mcp-eval)
[![crates.io](https://img.shields.io/crates/v/mcpeval)](https://crates.io/crates/mcpeval)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

**Measure how well an MCP server works for agents, gate it in CI, and turn real agent friction into findings you can fix.**

`mcpeval` scores any MCP server, over stdio or Streamable HTTP, with a
deterministic read-only battery. No LLM is in the loop: the same server and
manifest give the same result. Every lost point comes with the server-side fix.

A privacy-preserving shim records what real agent sessions cost, and repeated
failures become findings. A finding closes after three consecutive green
verification runs and reopens on regression.

> **Pre-1.0.** The CLI, on-disk schema, and manifest format can change between
> minor versions. Read the [changelog](CHANGELOG.md) before upgrading.

## Install

```sh
brew install cavi-ai/tap/mcpeval
npm install --global @cavi-ai/mcp-eval
cargo install mcpeval --locked
```

Each installs `mcpeval` and the demo server `mcpeval-demo`. Homebrew covers
macOS and Linux; npm also covers Windows x64. Both install the release archive
after checking its pinned SHA-256. See [Installation](docs/mcp-eval/source/pages/introduction/installation.md)
for release archives and source builds.

## First run

Score the bundled demo server against mcpeval's standard:

```sh
mcpeval score --server demo -- mcpeval-demo
# demo readiness 91/100 protocol=100 catalog=63 context=100 error-honesty=100 reliability=91 coverage=100 standard=mcpeval-standard/2
```

Scaffold a manifest from the live catalog, then gate on it:

```sh
mcpeval init --server demo --confirm-read-only \
  --output demo.manifest.json -- mcpeval-demo
mcpeval probe --server demo --manifest demo.manifest.json -- mcpeval-demo
# demo gate 25/25 passed
```

Break one behavior and read the fixed failure reason and its fix:

```sh
mcpeval probe --server demo --manifest demo.manifest.json \
  -- mcpeval-demo --broken stalled-cursor
# catalog-pagination pagination fail ... reason=pagination-stalled-cursor ...
mcpeval explain pagination-stalled-cursor
```

Replace `mcpeval-demo` with your server's command, or pass
`--url http://127.0.0.1:8080/mcp`. The [Quickstart](docs/mcp-eval/source/pages/introduction/quickstart.md)
walks through the rest.

## What you can do

| Goal | Guide |
| --- | --- |
| Score readiness and fix every lost point | [Readiness reporting](docs/mcp-eval/source/pages/guides/readiness-reporting.md) |
| Write and run manifest cases | [Capture and probe](docs/mcp-eval/source/pages/guides/capture-and-probe.md), [Manifest](docs/mcp-eval/source/pages/reference/manifest.md) |
| Gate pull requests and upload SARIF alerts | [Continuous integration](docs/mcp-eval/source/pages/guides/continuous-integration.md) |
| Compare servers or deployments on one manifest | [Comparing servers](docs/mcp-eval/source/pages/guides/readiness-reporting.md#comparing-servers) |
| Capture real agent sessions and promote findings | [Capture and probe](docs/mcp-eval/source/pages/guides/capture-and-probe.md), [Findings and verification](docs/mcp-eval/source/pages/guides/findings-and-verification.md) |
| Let an agent probe, score, and verify over MCP | [Serving findings and the agent loop](docs/mcp-eval/source/pages/guides/findings-and-verification.md#serving-findings-and-the-agent-loop) |
| Check that instructions match tool behavior | [Instruction coherence](docs/mcp-eval/source/pages/guides/instruction-coherence.md) |
| See how 33 popular public servers score | [State of MCP servers](docs/mcp-eval/source/pages/guides/state-of-mcp-servers.md) |
| Collect and replay your own corpus | [Corpus collection and replay](docs/mcp-eval/source/pages/guides/corpus-replay.md) |

## Safe by default

Probes are read-only. A mutating case needs a declared sandbox and
`--allow-mutation`, and the standard battery never calls a tool annotated as a
writer. Capture and probe records keep shaped arguments, fixed labels, and
counts, never raw argument values, response bodies, or error prose.

Records under `<MCPEVAL_HOME>/store/` are content-minimized, but annotation
notes are free-form prose: review or remove them before sharing, or use
`mcpeval share`, which omits them by default and refuses a snapshot its
redaction sweep flags. The salt, databases, manifests, and corpus files stay
private. See [Privacy and authorization](docs/mcp-eval/source/pages/security/privacy-and-authorization.md).

## Documentation

Hosted at [cavi-ai.xyz/docs/mcp-eval](https://cavi-ai.xyz/docs/mcp-eval/).

- Start: [Overview](docs/mcp-eval/source/pages/introduction/overview.md), [Installation](docs/mcp-eval/source/pages/introduction/installation.md), [Quickstart](docs/mcp-eval/source/pages/introduction/quickstart.md)
- Guides: [Capture and probe](docs/mcp-eval/source/pages/guides/capture-and-probe.md), [Readiness reporting](docs/mcp-eval/source/pages/guides/readiness-reporting.md), [Findings and verification](docs/mcp-eval/source/pages/guides/findings-and-verification.md), [Continuous integration](docs/mcp-eval/source/pages/guides/continuous-integration.md), [Instruction coherence](docs/mcp-eval/source/pages/guides/instruction-coherence.md), [Corpus collection and replay](docs/mcp-eval/source/pages/guides/corpus-replay.md), [Troubleshooting](docs/mcp-eval/source/pages/guides/troubleshooting.md)
- Reference: [CLI](docs/mcp-eval/source/pages/reference/cli.md), [Manifest](docs/mcp-eval/source/pages/reference/manifest.md), [Evaluation dimensions](docs/mcp-eval/source/pages/reference/evaluation-dimensions.md)
- Security and release: [Privacy and authorization](docs/mcp-eval/source/pages/security/privacy-and-authorization.md), [Version and support](docs/mcp-eval/source/pages/release/version-and-support.md)

Packages: [GitHub releases](https://github.com/cavi-ai/mcp-eval/releases/latest), [npm](https://www.npmjs.com/package/@cavi-ai/mcp-eval), [crates.io](https://crates.io/crates/mcpeval), [Homebrew tap](https://github.com/cavi-ai/homebrew-tap).

## Contributing

Bug reports and focused pull requests are welcome. [CONTRIBUTING.md](CONTRIBUTING.md)
covers development setup, quality gates, and the privacy rules for fixtures and
tests; [`.pr-gates`](.pr-gates) lists the commands CI runs. Report
vulnerabilities through [SECURITY.md](SECURITY.md).

Distributed under the [MIT License](LICENSE).

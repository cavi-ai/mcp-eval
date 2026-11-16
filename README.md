# MCP Eval

[![npm weekly downloads](https://img.shields.io/npm/dw/@cavi-ai/mcp-eval?label=npm%20downloads%2Fweek)](https://www.npmjs.com/package/@cavi-ai/mcp-eval)

![A corgi detective catches the same bug in disguise. Catch the repeat offenders. Reproduce the failure. Verify the fix.](assets/mcp-eval-hero.png)

**Find the failures your MCP server keeps sending back to agents.**

MCP Eval turns recurring tool errors from real sessions into findings you can reproduce, investigate and verify. Run a read-only readiness battery against a server, or capture session traffic to see which failures keep coming back. Findings include a server-side fix hint; they close after three consecutive passing verification runs and reopen if the problem returns.

## Try it

Install the CLI with one of these package managers:

```sh
brew install cavi-ai/tap/mcpeval
# or
npm install --global @cavi-ai/mcp-eval
# or
cargo install mcpeval --locked
```

Each installs `mcpeval` and `mcpeval-demo`. Run the included server through the readiness battery:

```sh
mcpeval score --server demo -- mcpeval-demo
```

The report breaks down protocol, catalog, context, error honesty, reliability and coverage, with findings and suggested fixes. Replace `mcpeval-demo` with your server command, or use `--url http://127.0.0.1:8080/mcp` for a Streamable HTTP server. See [Installation](docs/mcp-eval/source/pages/introduction/installation.md) for platform support and other install options, and [Quickstart](docs/mcp-eval/source/pages/introduction/quickstart.md) for the next steps.

## Turn recurring failures into fixes

Wrap a stdio server in your MCP client's configuration so MCP Eval can observe its calls:

```json
{
  "command": "/path/to/mcpeval",
  "args": ["shim", "--server", "myserver", "--", "/path/to/server", "serve"]
}
```

After sessions have run, index the records and promote recurring issues:

```sh
mcpeval index
mcpeval promote
mcpeval findings --format md
```

Promotion requires an issue to appear in at least two distinct sessions and meet the configured threshold. Use the resulting findings to investigate failures, create cases and verify a fix. The [capture guide](docs/mcp-eval/source/pages/guides/capture-and-probe.md) covers stdio and Streamable HTTP; [Findings and verification](docs/mcp-eval/source/pages/guides/findings-and-verification.md) explains the lifecycle.

## Evaluate on your terms

- Run the deterministic, read-only standard battery over stdio or Streamable HTTP. It does not use an LLM grader.
- Define manifest cases, probe a server and use the results in CI. Start with [Capture and probe](docs/mcp-eval/source/pages/guides/capture-and-probe.md) and [Continuous integration](docs/mcp-eval/source/pages/guides/continuous-integration.md).
- Give an MCP client access to evaluation tools for an agent-led workflow. Starting a server requires explicit spawn permission; see [Agent loop over MCP](docs/mcp-eval/source/pages/guides/agent-loop.md).
- Compare servers on one manifest, check instruction coherence, or collect and replay a corpus. See [Readiness reporting](docs/mcp-eval/source/pages/guides/readiness-reporting.md), [Instruction coherence](docs/mcp-eval/source/pages/guides/instruction-coherence.md) and [Corpus collection and replay](docs/mcp-eval/source/pages/guides/corpus-replay.md).

## Privacy and authorization

The standard battery is read-only and does not call tools annotated as writers. A mutating case requires a declared sandbox and `--allow-mutation`. Capture and probe records retain shaped arguments, fixed labels and counts, rather than raw argument values, response bodies or error prose.

Annotation notes are free-form text. Review or remove them before sharing; `mcpeval share` omits notes by default and refuses a snapshot flagged by its redaction sweep. Keep salts, databases, manifests and corpus files private. Read [Privacy and authorization](docs/mcp-eval/source/pages/security/privacy-and-authorization.md) before capturing or sharing records.

## Documentation

Browse the [hosted documentation](https://cavi-ai.xyz/docs/mcp-eval/) or start with [Overview](docs/mcp-eval/source/pages/introduction/overview.md), [CLI reference](docs/mcp-eval/source/pages/reference/cli.md), [Manifest reference](docs/mcp-eval/source/pages/reference/manifest.md), [Evaluation dimensions](docs/mcp-eval/source/pages/reference/evaluation-dimensions.md) and [Troubleshooting](docs/mcp-eval/source/pages/guides/troubleshooting.md).

MCP Eval is pre-1.0: CLI, on-disk schema and manifest formats can change between minor versions. Read the [changelog](CHANGELOG.md) before upgrading.

## Contributing

Bug reports and focused pull requests are welcome. [Contributing](CONTRIBUTING.md) covers development setup and privacy rules for fixtures and tests. Report vulnerabilities through [Security](SECURITY.md).

Distributed under the [MIT License](LICENSE).

# Installation

MCP Eval {{PRODUCT_VERSION}} installs the `mcpeval` and `mcpeval-demo` binaries from Homebrew, npm, or crates.io:

```sh
brew install cavi-ai/tap/mcpeval
npm install --global @cavi-ai/mcp-eval
cargo install mcpeval --locked
mcpeval --version
```

The version command must print `mcpeval {{PRODUCT_VERSION}}`. Building from a checked-out release source works too:

```sh
cargo build --release
./target/release/mcpeval --version
```

Homebrew (macOS and Linux, x64/arm64) and npm (also Windows x64) install the
prebuilt GitHub release archive after checking its pinned SHA-256. The
archives and their checksums are also on the release page.

Set `MCPEVAL_HOME` when you want a capture root other than the default:

```sh
export MCPEVAL_HOME="$PWD/.mcp-eval"
```

When `MCPEVAL_HOME` is unset, MCP Eval uses `$HOME/.mcp-eval` if `HOME` is available. Keep this directory private. Records in its `store/` child are content-minimized, but you must manually review or remove free-form annotation notes before sharing any store files.

For HTTP transports, no extra runtime service is installed. `mcpeval shim-http` listens only on the explicit loopback socket you provide and forwards to the validated upstream endpoint for the life of that process.

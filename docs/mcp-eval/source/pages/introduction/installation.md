# Installation

MCP Eval {{PRODUCT_VERSION}} is the Cargo package `mcpeval` and installs the `mcpeval` (and `mcpeval-demo`) binaries. Install from crates.io, or download a checksummed prebuilt binary from the release page:

```sh
cargo install mcpeval --locked
mcpeval --version
```

The version command must print `mcpeval {{PRODUCT_VERSION}}`. Building from a checked-out release source works too:

```sh
cargo build --release
./target/release/mcpeval --version
```

The repository also defines `@cavi-ai/mcp-eval` for npm and
`cavi-ai/tap/mcpeval` for Homebrew. Both consume the same checksummed GitHub
release archives; availability is determined by the public npm registry and
Homebrew tap rather than by the source definition alone.

Set `MCPEVAL_HOME` when you want a capture root other than the default:

```sh
export MCPEVAL_HOME="$PWD/.mcp-eval"
```

When `MCPEVAL_HOME` is unset, MCP Eval uses `$HOME/.mcp-eval` if `HOME` is available. Keep this directory private. Records in its `store/` child are content-minimized, but you must manually review or remove free-form annotation notes before sharing any store files.

For HTTP transports, no extra runtime service is installed. `mcpeval shim-http` listens only on the explicit loopback socket you provide and forwards to the validated upstream endpoint for the life of that process.

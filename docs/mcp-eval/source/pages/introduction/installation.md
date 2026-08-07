# Installation

MCP Eval {{PRODUCT_VERSION}} is the Cargo package `mcpeval` and installs the `mcpeval` binary. Build the checked-out release source with the stable Rust toolchain declared by the repository:

```sh
cargo build --release
./target/release/mcpeval --version
```

The version command must print `mcpeval {{PRODUCT_VERSION}}`.

Set `MCPEVAL_HOME` when you want a capture root other than the default:

```sh
export MCPEVAL_HOME="$PWD/.mcp-eval"
```

When `MCPEVAL_HOME` is unset, MCP Eval uses `$HOME/.mcp-eval` if `HOME` is available. Keep this directory private; only its `store/` child is designed as share-safe output.

For HTTP transports, no extra runtime service is installed. `mcpeval shim-http` listens only on the explicit loopback socket you provide and forwards to the validated upstream endpoint for the life of that process.

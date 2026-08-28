# Quickstart

No server of your own yet? The bundled `mcpeval-demo` binary is a real MCP server with a clean personality plus `--broken <aspect>` personalities that reproduce specific defects. Scaffold a manifest from its live catalog and run the deterministic battery:

```sh
mcpeval init --server demo --confirm-read-only \
  --output demo.manifest.json -- mcpeval-demo
mcpeval probe --server demo \
  --manifest demo.manifest.json -- mcpeval-demo
```

The probe scores readiness out of 100 and prints a per-category breakdown. Flip one flag to watch a fixed failure reason appear:

```sh
mcpeval probe --server demo --manifest demo.manifest.json \
  -- mcpeval-demo --broken stalled-cursor
```

To evaluate your own server, point the same flow at it:

```sh
mcpeval init --server demo --confirm-read-only -- your-mcp-server --flags
mcpeval probe --server demo -- your-mcp-server --flags
```

After the captured sessions contain representative traffic, rebuild the index, promote supported recurring failures, and view the sanitized findings:

```sh
mcpeval index
mcpeval promote
mcpeval findings --format agent
```

Wrap a newline-delimited JSON-RPC MCP server and let the client connect through the shim:

```sh
mcpeval shim --server demo -- your-mcp-server --flags
```

Verify one promoted finding with exactly one declared case:

```sh
mcpeval verify --finding finding-0123456789abcdef \
  --case repeat-read --manifest mcp-eval.manifest.json \
  -- your-mcp-server --flags
```

Use identifiers emitted by your own `mcpeval findings` output. The example finding ID only demonstrates the CLI shape.

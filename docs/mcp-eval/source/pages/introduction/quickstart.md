# Quickstart

Wrap a newline-delimited JSON-RPC MCP server and let the client connect through the shim:

```sh
mcpeval shim --server demo -- your-mcp-server --flags
```

After the captured sessions contain representative traffic, rebuild the index, promote supported recurring failures, and view the sanitized findings:

```sh
mcpeval index
mcpeval promote
mcpeval findings --format agent
```

Create `mcp-eval.manifest.json`, then run its deterministic read-only cases:

```sh
mcpeval probe --server demo \
  --manifest mcp-eval.manifest.json -- your-mcp-server --flags
```

Verify one promoted finding with exactly one declared case:

```sh
mcpeval verify --finding finding-0123456789abcdef \
  --case repeat-read --manifest mcp-eval.manifest.json \
  -- your-mcp-server --flags
```

Use identifiers emitted by your own `mcpeval findings` output. The example finding ID only demonstrates the CLI shape.

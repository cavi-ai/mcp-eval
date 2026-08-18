# Capture and probe

## Capture stdio traffic

Place the server command after `--` so MCP Eval can proxy it without interpreting server flags:

```sh
mcpeval shim --server demo -- your-mcp-server --flags
```

The stdio shim expects newline-delimited JSON-RPC. It forwards invalid frames unchanged but records only a content-free `unparsed/outbound` or `unparsed/inbound` event.

## Capture Streamable HTTP traffic

Start a loopback proxy and configure the MCP client to use its listen URL:

```sh
mcpeval shim-http --server demo \
  --listen 127.0.0.1:8090 \
  --upstream http://127.0.0.1:8080/mcp
```

Remote upstreams require HTTPS and `--allow-remote-http`. Endpoint URLs cannot contain credentials, query strings, or fragments, and redirects are disabled.

## Run probes

Run all manifest cases over stdio, or select one dimension with `--probe`:

```sh
mcpeval probe --server demo --manifest mcp-eval.manifest.json \
  --probe discovery-cost -- your-mcp-server --flags
```

Use `--url` for a Streamable HTTP endpoint:

```sh
mcpeval probe --server demo --manifest mcp-eval.manifest.json \
  --url http://127.0.0.1:8080/mcp
```

Mutating cases are rejected unless each case names a declared sandbox and the invocation includes `--allow-mutation`. The flag authorizes only manifest-declared cases; it does not turn capture into a mutation capability.

For an eligible promoted finding with empty shaped arguments, `generate` can write a deterministic read-only manifest using the supplemental `degradation-over-n` probe:

```sh
mcpeval generate --finding finding-0123456789abcdef \
  --output generated.manifest.json --confirm-read-only
```

`--confirm-read-only` is an operator attestation about the selected tool, not mutation authorization. Add `--force` only when replacing the requested output file is intentional.

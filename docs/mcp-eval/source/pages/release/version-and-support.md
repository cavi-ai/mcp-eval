# Version and support

These pages describe MCP Eval {{PRODUCT_VERSION}}, released from tag `{{RELEASE_TAG}}` at commit `{{RELEASE_COMMIT}}`. They are published as a static site at the repository's GitHub Pages URL, where every released version stays browsable and the newest is the default; the per-release archive `mcp-eval-docs-{{RELEASE_TAG}}.tar.gz` and its `.sha256` companion remain the byte-identical distribution artifacts.

MCP Eval is under active development. Before 1.0, the CLI, on-disk schema, and manifest format may change. Keep manifests with the release that validates them and review the repository changelog before upgrading.

For defects, include the MCP Eval version, sanitized command shape, and only the necessary content-minimized records from `<MCPEVAL_HOME>/store/` after manually reviewing or removing every annotation note. Do not attach the capture root, `.salt`, raw payloads, authorization values, manifests containing operational arguments, or other credentials. Follow the repository security policy for vulnerability reports.

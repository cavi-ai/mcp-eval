# Corpus collection and verification

The target file, collection, replay, and offline evidence workflow is documented
in [Corpus collection and replay](../../docs/mcp-eval/source/pages/guides/corpus-replay.md).

Run deterministic fixture coverage with `npm run test:corpus`. This lane does
not download public servers or collect comparative measurements. Build both
release binaries first, or set `MCPEVAL_CORPUS_TEST_BINARY` to a built debug
evaluator; the native fixture uses the demo beside that executable.

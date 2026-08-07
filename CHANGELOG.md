# Changelog

All notable changes to this project are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and the project uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Stdio MCP shim with privacy-preserving call capture.
- JSONL store, SQLite index, annotations, issue promotion, and findings reports.
- Deterministic `degradation-over-n` and `instruction-fidelity` probes.
- Strict `mcp-eval.manifest.json` validation and explicit sandbox authorization
  for mutating probes.
- Clean and broken fixture-server integration coverage.

### Security

- Probe calls remain read-only by default and use the existing sanitized
  persistence boundary.

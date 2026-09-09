#!/usr/bin/env python3
"""Every artifact in this repo ships under one version. Prove it.

One tag (`v*`) publishes the binaries, the npm package, and the crate, so a
version that disagrees between manifests means the tag ships a mislabelled
artifact. This is the gate: it exits non-zero and names every disagreement.

`distribution/release.json` and `Formula/mcpeval.rb` are deliberately not
checked here. They record the last *published* release (archive digests are
only known after the tag builds) and lag the source version until the
post-release pull request refreshes them; `scripts/distribution/verify.mjs`
owns that contract.
"""
from __future__ import annotations

import json
import pathlib
import re
import sys

REPO = pathlib.Path(__file__).resolve().parent.parent
CRATE = "mcpeval"
NPM_SCOPE = "@cavi-ai/"


def cargo_version() -> str:
    manifest = (REPO / "Cargo.toml").read_text()
    match = re.search(r'^\[package\][^\[]*?^version = "([^"]+)"', manifest, re.M | re.S)
    if not match:
        sys.exit("could not read [package] version from Cargo.toml")
    return match.group(1)


def lockfile_version(expected: str) -> list[str]:
    """`cargo publish --locked` and the release build both refuse a lockfile
    whose own entry disagrees with Cargo.toml; catch it on the PR instead."""
    text = (REPO / "Cargo.lock").read_text()
    match = re.search(rf'^name = "{CRATE}"\nversion = "([^"]+)"', text, re.M)
    if not match:
        return [f"Cargo.lock: no entry for {CRATE}"]
    if match.group(1) != expected:
        return [f"Cargo.lock: {CRATE} {match.group(1)} != {expected}"]
    return []


def package_version(expected: str) -> list[str]:
    data = json.loads((REPO / "package.json").read_text())
    problems = []
    if data.get("version") != expected:
        problems.append(f"package.json: {data.get('version')} != {expected}")
    name = data.get("name", "")
    if not name.startswith(NPM_SCOPE):
        problems.append(f"package.json: {name} is not under the {NPM_SCOPE} scope")
    return problems


def main() -> int:
    expected = cargo_version()
    problems = [*lockfile_version(expected), *package_version(expected)]
    if problems:
        print("\n".join(problems), file=sys.stderr)
        return 1
    print(f"ok: every manifest agrees on {expected}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

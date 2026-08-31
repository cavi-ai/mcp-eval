#!/usr/bin/env node
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const MODULE_PATH = fileURLToPath(import.meta.url);
const DEFAULT_ROOT = path.resolve(path.dirname(MODULE_PATH), "../..");
const SHA256 = /^[a-f0-9]{64}$/u;
const COMMIT = /^[a-f0-9]{40}$/u;
const TARGETS = {
  "darwin-arm64": ["aarch64-apple-darwin", "tar.gz"],
  "darwin-x64": ["x86_64-apple-darwin", "tar.gz"],
  "linux-arm64": ["aarch64-unknown-linux-gnu", "tar.gz"],
  "linux-x64": ["x86_64-unknown-linux-gnu", "tar.gz"],
  "win32-x64": ["x86_64-pc-windows-msvc", "zip"],
};

function releaseUrl(manifest, asset) {
  return `https://github.com/${manifest.repository}/releases/download/${manifest.tag}/${asset.archive}`;
}

export function verifyChecksumCompanion(text, asset) {
  const match = /^([a-f0-9]{64})  ([^\r\n]+)\r?\n?$/u.exec(text);
  assert.ok(match, `invalid published checksum: ${asset.archive}`);
  assert.equal(match[1], asset.sha256, `published checksum digest mismatch: ${asset.archive}`);
  assert.equal(match[2], asset.archive, `published checksum filename mismatch: ${asset.archive}`);
}

export function renderFormula(manifest) {
  const asset = (key) => manifest.assets[key];
  return `# typed: false
# frozen_string_literal: true

# Generated from distribution/release.json by scripts/distribution/verify.mjs.
class Mcpeval < Formula
  desc "Privacy-preserving MCP friction capture and deterministic evaluation"
  homepage "https://github.com/cavi-ai/mcp-eval"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/cavi-ai/mcp-eval/releases/download/${manifest.tag}/${asset("darwin-arm64").archive}"
      sha256 "${asset("darwin-arm64").sha256}"
    end
    on_intel do
      url "https://github.com/cavi-ai/mcp-eval/releases/download/${manifest.tag}/${asset("darwin-x64").archive}"
      sha256 "${asset("darwin-x64").sha256}"
    end
  end

  on_linux do
    on_arm do
      url "https://github.com/cavi-ai/mcp-eval/releases/download/${manifest.tag}/${asset("linux-arm64").archive}"
      sha256 "${asset("linux-arm64").sha256}"
    end
    on_intel do
      url "https://github.com/cavi-ai/mcp-eval/releases/download/${manifest.tag}/${asset("linux-x64").archive}"
      sha256 "${asset("linux-x64").sha256}"
    end
  end

  def install
    bin.install "mcpeval"
    bin.install "mcpeval-demo"
  end

  test do
    assert_match "mcpeval ${manifest.version}", shell_output("#{bin}/mcpeval --version")
  end
end
`;
}

function git(root, ...args) {
  const result = spawnSync("git", args, { cwd: root, encoding: "utf8" });
  assert.equal(result.status, 0, result.stderr);
  return result.stdout.trim();
}

async function verifyOnline(manifest) {
  for (const asset of Object.values(manifest.assets)) {
    const url = releaseUrl(manifest, asset);
    const checksumResponse = await fetch(`${url}.sha256`);
    assert.equal(checksumResponse.status, 200, `missing checksum companion: ${asset.archive}`);
    verifyChecksumCompanion(await checksumResponse.text(), asset);
    const assetResponse = await fetch(url);
    assert.equal(assetResponse.status, 200, `missing release asset: ${asset.archive}`);
    const actual = createHash("sha256").update(Buffer.from(await assetResponse.arrayBuffer())).digest("hex");
    assert.equal(actual, asset.sha256, `published asset mismatch: ${asset.archive}`);
  }
}

export async function verifyDistribution({ root = DEFAULT_ROOT, online = false } = {}) {
  const manifest = JSON.parse(await readFile(path.join(root, "distribution/release.json"), "utf8"));
  const npmPackage = JSON.parse(await readFile(path.join(root, "package.json"), "utf8"));
  const cargo = await readFile(path.join(root, "Cargo.toml"), "utf8");
  const cargoVersion = /^version = "([^"]+)"$/mu.exec(cargo)?.[1];

  assert.equal(manifest.schemaVersion, 1);
  assert.equal(manifest.package, "@cavi-ai/mcp-eval");
  assert.equal(manifest.repository, "cavi-ai/mcp-eval");
  assert.equal(manifest.tag, `v${manifest.version}`);
  assert.match(manifest.commit, COMMIT);
  assert.equal(npmPackage.name, manifest.package);
  assert.equal(npmPackage.version, manifest.version);
  assert.equal(cargoVersion, manifest.version);
  assert.equal(git(root, "rev-list", "-n", "1", manifest.tag), manifest.commit);
  assert.deepEqual(Object.keys(manifest.assets).sort(), Object.keys(TARGETS).sort());

  for (const [key, [target, extension]] of Object.entries(TARGETS)) {
    const asset = manifest.assets[key];
    assert.equal(asset.target, target);
    assert.equal(asset.archive, `mcpeval-${target}.${extension}`);
    assert.match(asset.sha256, SHA256);
  }

  assert.equal(
    await readFile(path.join(root, "Formula/mcpeval.rb"), "utf8"),
    renderFormula(manifest),
    "Formula/mcpeval.rb is not generated from distribution/release.json",
  );
  const ruby = spawnSync("ruby", ["-c", path.join(root, "Formula/mcpeval.rb")], { encoding: "utf8" });
  assert.equal(ruby.status, 0, ruby.stderr);

  if (online) await verifyOnline(manifest);
  return {
    version: manifest.version,
    tag: manifest.tag,
    commit: manifest.commit,
    npmPackage: manifest.package,
    assets: Object.fromEntries(Object.entries(manifest.assets).map(([key, asset]) => [key, asset.sha256])),
    formulaAssets: 4,
  };
}

async function main() {
  const result = await verifyDistribution({ online: process.argv.includes("--online") });
  console.log(JSON.stringify(result, null, 2));
}

if (process.argv[1] && path.resolve(process.argv[1]) === MODULE_PATH) {
  main().catch((error) => {
    console.error(error.stack ?? error.message);
    process.exitCode = 1;
  });
}

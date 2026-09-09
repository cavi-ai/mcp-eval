#!/usr/bin/env node
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { TARGETS, verifyChecksumCompanion } from "./verify.mjs";

const MODULE_PATH = fileURLToPath(import.meta.url);
const DEFAULT_ROOT = path.resolve(path.dirname(MODULE_PATH), "../..");
const TAG = /^v(\d+\.\d+\.\d+)$/u;
const COMMIT = /^[a-f0-9]{40}$/u;

/**
 * Build the distribution manifest (`distribution/release.json`) from a
 * directory holding one built archive plus `.sha256` companion per target.
 * The manifest is what the npm installer and the Homebrew formula pin.
 */
export async function buildReleaseManifest({ dist, tag, commit, root = DEFAULT_ROOT }) {
  const version = TAG.exec(tag)?.[1];
  assert.ok(version, `release tag must be vX.Y.Z: ${tag}`);
  assert.match(commit, COMMIT, "release commit must be a full lowercase SHA");
  const npmPackage = JSON.parse(await readFile(path.join(root, "package.json"), "utf8"));
  const repository = /github\.com\/([^/]+\/[^/.]+)/u.exec(npmPackage.repository?.url ?? "")?.[1];
  assert.ok(repository, "package.json repository.url must name the GitHub repository");

  const assets = {};
  for (const [key, [target, extension]] of Object.entries(TARGETS)) {
    const archive = `mcpeval-${target}.${extension}`;
    const bytes = await readFile(path.join(dist, archive));
    const asset = { target, archive, size: bytes.length, sha256: createHash("sha256").update(bytes).digest("hex") };
    verifyChecksumCompanion(await readFile(path.join(dist, `${archive}.sha256`), "utf8"), asset);
    assets[key] = asset;
  }
  return { schemaVersion: 1, package: npmPackage.name, repository, version, tag, commit, assets };
}

function value(args, flag) {
  const index = args.indexOf(flag);
  if (index === -1 || !args[index + 1]) throw new Error(`${flag} is required`);
  return args[index + 1];
}

async function main() {
  const args = process.argv.slice(2);
  const manifest = await buildReleaseManifest({
    dist: path.resolve(value(args, "--dist")),
    tag: value(args, "--tag"),
    commit: value(args, "--commit"),
  });
  process.stdout.write(`${JSON.stringify(manifest, null, 2)}\n`);
}

if (process.argv[1] && path.resolve(process.argv[1]) === MODULE_PATH) {
  main().catch((error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
}

#!/usr/bin/env node
import assert from "node:assert/strict";
import { mkdir, mkdtemp, readdir, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";

const ROOT = path.resolve(import.meta.dirname, "../..");

function run(command, args, options = {}) {
  const result = spawnSync(command, args, {
    encoding: "utf8",
    shell: process.platform === "win32" && command.toLowerCase().endsWith(".cmd"),
    ...options,
  });
  assert.equal(result.status, 0, [result.stdout, result.stderr].filter(Boolean).join("\n"));
  return result;
}

async function main() {
  const temporary = await mkdtemp(path.join(os.tmpdir(), "mcpeval-package-e2e-"));
  const packageDirectory = path.join(temporary, "package");
  const prefix = path.join(temporary, "prefix");
  const npmCache = path.join(temporary, "npm-cache");
  const npm = process.platform === "win32" ? "npm.cmd" : "npm";
  const environment = { ...process.env, npm_config_cache: npmCache };

  try {
    await mkdir(packageDirectory, { recursive: true });
    run(npm, ["pack", "--pack-destination", packageDirectory], {
      cwd: ROOT,
      env: environment,
    });
    const packages = (await readdir(packageDirectory)).filter((file) => file.endsWith(".tgz"));
    assert.deepEqual(packages, ["cavi-ai-mcp-eval-0.1.0.tgz"]);
    const packagePath = path.join(packageDirectory, packages[0]);

    run(npm, ["install", "--global", "--prefix", prefix, packagePath], {
      cwd: temporary,
      env: environment,
    });

    const executable = process.platform === "win32"
      ? path.join(prefix, "mcpeval.cmd")
      : path.join(prefix, "bin", "mcpeval");
    const result = run(executable, ["--version"], { cwd: temporary });
    assert.equal(result.stdout.trim(), "mcpeval 0.1.0");
    console.log(`verified ${packages[0]} on ${process.platform}-${process.arch}: ${result.stdout.trim()}`);
  } finally {
    await rm(temporary, { recursive: true, force: true });
  }
}

main().catch((error) => {
  console.error(error.stack ?? error.message);
  process.exitCode = 1;
});

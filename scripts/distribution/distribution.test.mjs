import assert from "node:assert/strict";
import { chmod, copyFile, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import test from "node:test";

const ROOT = path.resolve(import.meta.dirname, "../..");

const RELEASE_ASSETS = {
  "darwin-arm64": "d6ab42cd065a536b082730a1054d71fc86f863eab81608c46261f2a9350aa6f2",
  "darwin-x64": "fe32b9bcb10f54209a3d819614049b2c8bb12ef4182808deb9152e5d6b3769f8",
  "linux-arm64": "dfa94d7e8c553196d857e017e76b221c8d57d6f5703acc4e781454cbdd68df6f",
  "linux-x64": "6d2f2c5df822e9be6f786ab41512d869d41fb9170c36f3f6f60f48b02caedb7e",
  "win32-x64": "c1cf80f4b0a70739de2b44d3322c9888da0ebb91fc05ed2285b0a8ae70340055",
};

test("published checksum parser accepts the Unix and Windows line endings", async () => {
  const { verifyChecksumCompanion } = await import("./verify.mjs");
  const archive = "mcpeval-x86_64-pc-windows-msvc.zip";
  const sha256 = RELEASE_ASSETS["win32-x64"];
  assert.doesNotThrow(() => verifyChecksumCompanion(`${sha256}  ${archive}`, { archive, sha256 }));
  assert.doesNotThrow(() => verifyChecksumCompanion(`${sha256}  ${archive}\n`, { archive, sha256 }));
});

test("distribution contract pins the published v0.1.0 release", async () => {
  const { verifyDistribution } = await import("./verify.mjs");
  assert.deepEqual(await verifyDistribution({ root: ROOT }), {
    version: "0.1.0",
    tag: "v0.1.0",
    commit: "ffc0b2d0d0922f3f4daa246effcfa25fe7be349a",
    npmPackage: "@cavi-ai/mcp-eval",
    assets: RELEASE_ASSETS,
    formulaAssets: 4,
  });
});

test("npm dry-run contains only the launcher, installer, release contract, and package docs", () => {
  const packed = spawnSync("npm", ["pack", "--dry-run", "--json", "--ignore-scripts"], {
    cwd: ROOT,
    encoding: "utf8",
    env: { ...process.env, npm_config_cache: path.join(os.tmpdir(), "mcpeval-npm-cache") },
  });
  assert.equal(packed.status, 0, packed.stderr);
  const [result] = JSON.parse(packed.stdout);
  assert.equal(result.filename, "cavi-ai-mcp-eval-0.1.0.tgz");
  assert.deepEqual(
    result.files.map(({ path: file }) => file).sort(),
    [
      "LICENSE",
      "README.md",
      "distribution/release.json",
      "npm/install.mjs",
      "npm/run.mjs",
      "package.json",
    ],
  );
});

test("npm launcher forwards arguments and the binary exit status", { skip: process.platform === "win32" }, async (context) => {
  const temporary = await mkdtemp(path.join(os.tmpdir(), "mcpeval-npm-run-"));
  context.after(() => rm(temporary, { recursive: true, force: true }));
  await mkdir(path.join(temporary, "npm/vendor"), { recursive: true });
  await copyFile(path.join(ROOT, "npm/run.mjs"), path.join(temporary, "npm/run.mjs"));
  const binary = path.join(temporary, "npm/vendor/mcpeval");
  await writeFile(binary, "#!/bin/sh\nprintf '%s\\n' \"$*\"\nexit 23\n");
  await chmod(binary, 0o755);

  const launched = spawnSync(process.execPath, [path.join(temporary, "npm/run.mjs"), "probe", "--brief"], {
    encoding: "utf8",
  });
  assert.equal(launched.stdout, "probe --brief\n");
  assert.equal(launched.status, 23);
});

test("Homebrew staging updates the tap formula and index idempotently", async (context) => {
  const temporary = await mkdtemp(path.join(os.tmpdir(), "mcpeval-homebrew-stage-"));
  context.after(() => rm(temporary, { recursive: true, force: true }));
  const tapRoot = path.join(temporary, "tap");
  await mkdir(path.join(tapRoot, "Formula"), { recursive: true });
  await writeFile(path.join(tapRoot, "README.md"), `# cavi-ai/homebrew-tap

## Formulae

| Formula | Installs | Upstream |
|---|---|---|
| \`bobby-browser\` | \`bobby\` | [cavi-ai/bobby-browser](https://github.com/cavi-ai/bobby-browser) |

Formulae pull checksummed release binaries.
`);
  const { stageHomebrewTap } = await import("./stage-homebrew.mjs");

  const first = await stageHomebrewTap({ sourceRoot: ROOT, tapRoot });
  const readmeAfterFirst = await readFile(path.join(tapRoot, "README.md"), "utf8");
  assert.deepEqual(first, { formulaChanged: true, readmeChanged: true });
  assert.equal(
    await readFile(path.join(tapRoot, "Formula/mcpeval.rb"), "utf8"),
    await readFile(path.join(ROOT, "Formula/mcpeval.rb"), "utf8"),
  );
  assert.match(
    readmeAfterFirst,
    /\| `mcpeval` \| `mcpeval`, `mcpeval-demo` \| \[cavi-ai\/mcp-eval\]\(https:\/\/github\.com\/cavi-ai\/mcp-eval\) \|/u,
  );

  assert.deepEqual(await stageHomebrewTap({ sourceRoot: ROOT, tapRoot }), {
    formulaChanged: false,
    readmeChanged: false,
  });
  assert.equal(await readFile(path.join(tapRoot, "README.md"), "utf8"), readmeAfterFirst);
});

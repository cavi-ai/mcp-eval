import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { chmod, copyFile, mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import test from "node:test";

const ROOT = path.resolve(import.meta.dirname, "../..");

// The committed manifest records the last published release; the contract
// tests derive their expectations from it so a refresh never rewrites them.
const MANIFEST = JSON.parse(await readFile(path.join(ROOT, "distribution/release.json"), "utf8"));
const PACKAGE = JSON.parse(await readFile(path.join(ROOT, "package.json"), "utf8"));
const RELEASE_ASSETS = Object.fromEntries(
  Object.entries(MANIFEST.assets).map(([key, asset]) => [key, asset.sha256]),
);

test("published checksum parser accepts the Unix and Windows line endings", async () => {
  const { verifyChecksumCompanion } = await import("./verify.mjs");
  const archive = "mcpeval-x86_64-pc-windows-msvc.zip";
  const sha256 = RELEASE_ASSETS["win32-x64"];
  assert.doesNotThrow(() => verifyChecksumCompanion(`${sha256}  ${archive}`, { archive, sha256 }));
  assert.doesNotThrow(() => verifyChecksumCompanion(`${sha256}  ${archive}\n`, { archive, sha256 }));
});

test("distribution contract pins the published release", async () => {
  const { verifyDistribution } = await import("./verify.mjs");
  assert.match(MANIFEST.tag, /^v\d+\.\d+\.\d+$/u);
  assert.match(MANIFEST.commit, /^[a-f0-9]{40}$/u);
  assert.deepEqual(await verifyDistribution({ root: ROOT }), {
    version: MANIFEST.version,
    tag: MANIFEST.tag,
    commit: MANIFEST.commit,
    npmPackage: "@cavi-ai/mcp-eval",
    assets: RELEASE_ASSETS,
    formulaAssets: 4,
  });
  assert.deepEqual(await verifyDistribution({ root: ROOT, strict: PACKAGE.version === MANIFEST.version }), {
    version: MANIFEST.version,
    tag: MANIFEST.tag,
    commit: MANIFEST.commit,
    npmPackage: "@cavi-ai/mcp-eval",
    assets: RELEASE_ASSETS,
    formulaAssets: 4,
  });
});

async function sourceTreeWithVersion(version) {
  const temporary = await mkdtemp(path.join(os.tmpdir(), "mcpeval-version-lag-"));
  await mkdir(path.join(temporary, "distribution"), { recursive: true });
  await mkdir(path.join(temporary, "Formula"), { recursive: true });
  const cargo = await readFile(path.join(ROOT, "Cargo.toml"), "utf8");
  await writeFile(path.join(temporary, "Cargo.toml"), cargo.replace(/^version = "[^"]+"$/mu, `version = "${version}"`));
  await writeFile(path.join(temporary, "package.json"), JSON.stringify({ ...PACKAGE, version }, null, 2));
  await copyFile(path.join(ROOT, "distribution/release.json"), path.join(temporary, "distribution/release.json"));
  await copyFile(path.join(ROOT, "Formula/mcpeval.rb"), path.join(temporary, "Formula/mcpeval.rb"));
  return temporary;
}

test("a bumped source version is accepted until the post-release refresh, but never for publishing", async (context) => {
  const { verifyDistribution } = await import("./verify.mjs");
  const next = MANIFEST.version.replace(/\d+$/u, (patch) => String(Number(patch) + 1));
  const bumped = await sourceTreeWithVersion(next);
  context.after(() => rm(bumped, { recursive: true, force: true }));
  const result = await verifyDistribution({ root: bumped, gitRoot: ROOT });
  assert.equal(result.version, MANIFEST.version);
  await assert.rejects(
    verifyDistribution({ root: bumped, gitRoot: ROOT, strict: true }),
    /must equal the distribution manifest version/u,
  );

  const behind = await sourceTreeWithVersion("0.0.1");
  context.after(() => rm(behind, { recursive: true, force: true }));
  await assert.rejects(verifyDistribution({ root: behind, gitRoot: ROOT }), /is behind the published distribution/u);
});

test("release manifest is derived from the built archives and their checksum companions", async (context) => {
  const { buildReleaseManifest } = await import("./release-manifest.mjs");
  const { TARGETS } = await import("./verify.mjs");
  const dist = await mkdtemp(path.join(os.tmpdir(), "mcpeval-release-manifest-"));
  context.after(() => rm(dist, { recursive: true, force: true }));
  const expected = {};
  for (const [key, [target, extension]] of Object.entries(TARGETS)) {
    const archive = `mcpeval-${target}.${extension}`;
    const bytes = Buffer.from(`fixture archive for ${target}`);
    const sha256 = createHash("sha256").update(bytes).digest("hex");
    await writeFile(path.join(dist, archive), bytes);
    await writeFile(path.join(dist, `${archive}.sha256`), `${sha256}  ${archive}\n`);
    expected[key] = { target, archive, size: bytes.length, sha256 };
  }
  const commit = "a".repeat(40);
  const manifest = await buildReleaseManifest({ dist, tag: "v9.9.9", commit, root: ROOT });
  assert.deepEqual(manifest, {
    schemaVersion: 1,
    package: "@cavi-ai/mcp-eval",
    repository: "cavi-ai/mcp-eval",
    version: "9.9.9",
    tag: "v9.9.9",
    commit,
    assets: expected,
  });
  assert.deepEqual(Object.keys(manifest), ["schemaVersion", "package", "repository", "version", "tag", "commit", "assets"]);

  const { renderFormula } = await import("./verify.mjs");
  const formula = renderFormula(manifest);
  assert.match(formula, /releases\/download\/v9\.9\.9\/mcpeval-aarch64-apple-darwin\.tar\.gz/u);
  assert.match(formula, /assert_match "mcpeval 9\.9\.9"/u);

  await writeFile(path.join(dist, "mcpeval-x86_64-unknown-linux-gnu.tar.gz.sha256"), `${"0".repeat(64)}  mcpeval-x86_64-unknown-linux-gnu.tar.gz\n`);
  await assert.rejects(buildReleaseManifest({ dist, tag: "v9.9.9", commit, root: ROOT }), /checksum digest mismatch/u);
  await assert.rejects(buildReleaseManifest({ dist, tag: "9.9.9", commit, root: ROOT }), /release tag must be vX\.Y\.Z/u);
});

test("render-formula reproduces the committed formula from the committed manifest", async () => {
  const { renderFormulaFile } = await import("./render-formula.mjs");
  assert.equal(await renderFormulaFile(), await readFile(path.join(ROOT, "Formula/mcpeval.rb"), "utf8"));
});

test("npm dry-run contains only the launcher, installer, release contract, and package docs", () => {
  const packed = spawnSync("npm", ["pack", "--dry-run", "--json", "--ignore-scripts"], {
    cwd: ROOT,
    encoding: "utf8",
    env: { ...process.env, npm_config_cache: path.join(os.tmpdir(), "mcpeval-npm-cache") },
  });
  assert.equal(packed.status, 0, packed.stderr);
  const [result] = JSON.parse(packed.stdout);
  assert.equal(result.filename, `cavi-ai-mcp-eval-${PACKAGE.version}.tgz`);
  assert.deepEqual(
    result.files.map(({ path: file }) => file).sort(),
    [
      "LICENSE",
      "README.md",
      "distribution/release.json",
      "npm/demo.mjs",
      "npm/install.mjs",
      "npm/launch.mjs",
      "npm/run.mjs",
      "package.json",
    ],
  );
});

test("npm exposes both release binaries as commands", () => {
  assert.deepEqual(PACKAGE.bin, { mcpeval: "npm/run.mjs", "mcpeval-demo": "npm/demo.mjs" });
});

for (const [entry, binaryName] of [["run.mjs", "mcpeval"], ["demo.mjs", "mcpeval-demo"]]) {
  test(`npm ${entry} launches ${binaryName} with its arguments and exit status`, { skip: process.platform === "win32" }, async (context) => {
    const temporary = await mkdtemp(path.join(os.tmpdir(), "mcpeval-npm-run-"));
    context.after(() => rm(temporary, { recursive: true, force: true }));
    await mkdir(path.join(temporary, "npm/vendor"), { recursive: true });
    for (const file of ["run.mjs", "demo.mjs", "launch.mjs"]) {
      await copyFile(path.join(ROOT, "npm", file), path.join(temporary, "npm", file));
    }
    for (const name of ["mcpeval", "mcpeval-demo"]) {
      const binary = path.join(temporary, "npm/vendor", name);
      await writeFile(binary, `#!/bin/sh\nprintf '${name} %s\\n' "$*"\nexit 23\n`);
      await chmod(binary, 0o755);
    }

    const launched = spawnSync(process.execPath, [path.join(temporary, "npm", entry), "probe", "--brief"], {
      encoding: "utf8",
    });
    assert.equal(launched.stdout, `${binaryName} probe --brief\n`);
    assert.equal(launched.status, 23);
  });
}

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

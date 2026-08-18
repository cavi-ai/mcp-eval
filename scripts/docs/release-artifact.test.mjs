import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { gunzipSync } from "node:zlib";

const IDENTITY = {
  version: "0.1.0",
  tag: "v0.1.0",
  commit: "0123456789abcdef0123456789abcdef01234567",
  sourceDateEpoch: 1700000000,
};

function tarNames(archive) {
  const tar = gunzipSync(archive);
  const names = [];
  for (let offset = 0; offset + 512 <= tar.length;) {
    const header = tar.subarray(offset, offset + 512);
    if (header.every((byte) => byte === 0)) break;
    const value = (start, length) => header.subarray(start, start + length).toString().replace(/\0.*$/u, "");
    const name = value(0, 100);
    const prefix = value(345, 155);
    names.push(prefix ? `${prefix}/${name}` : name);
    const size = Number.parseInt(value(124, 12).trim() || "0", 8);
    offset += 512 + Math.ceil(size / 512) * 512;
  }
  return names;
}

test("release artifact, checksum, and exact envelope are deterministic", async (context) => {
  const temporary = await mkdtemp(path.join(os.tmpdir(), "mcpeval-docs-release-"));
  context.after(() => rm(temporary, { recursive: true, force: true }));
  const { buildDocumentation } = await import("./build.mjs");
  const { createProductDocsReleaseArtifact } = await import("./release-artifact.mjs");
  const docsRoot = path.join(temporary, "docs");
  await buildDocumentation({ ...IDENTITY, outputRoot: docsRoot });
  const results = [];
  for (const name of ["one", "two"]) {
    results.push(await createProductDocsReleaseArtifact({
      ...IDENTITY,
      docsRoot,
      outputDirectory: path.join(temporary, name),
      repository: "cavi-ai/mcp-eval",
    }));
  }
  const firstBytes = await readFile(results[0].artifactPath);
  assert.deepEqual(firstBytes, await readFile(results[1].artifactPath));
  const sha256 = createHash("sha256").update(firstBytes).digest("hex");
  assert.deepEqual(results[0].envelope, {
    schemaVersion: 1,
    slug: "mcp-eval",
    kind: "product-docs",
    version: "0.1.0",
    tag: "v0.1.0",
    repository: "cavi-ai/mcp-eval",
    commit: IDENTITY.commit,
    artifact: {
      url: "https://github.com/cavi-ai/mcp-eval/releases/download/v0.1.0/mcp-eval-docs-v0.1.0.tar.gz",
      sha256,
      format: "tar.gz",
    },
  });
  assert.equal(
    await readFile(results[0].checksumPath, "utf8"),
    `${sha256}  mcp-eval-docs-v0.1.0.tar.gz\n`,
  );
  const names = tarNames(firstBytes);
  assert.ok(names.includes("cavi-release.json"));
  assert.ok(names.every((name) => name === "cavi-release.json" || name.startsWith("docs/mcp-eval/v0.1.0/")));
});

test("release artifact rejects documentation built for another commit", async (context) => {
  const temporary = await mkdtemp(path.join(os.tmpdir(), "mcpeval-docs-provenance-"));
  context.after(() => rm(temporary, { recursive: true, force: true }));
  const { buildDocumentation } = await import("./build.mjs");
  const { createProductDocsReleaseArtifact } = await import("./release-artifact.mjs");
  const docsRoot = path.join(temporary, "docs");
  await buildDocumentation({ ...IDENTITY, outputRoot: docsRoot });
  await assert.rejects(createProductDocsReleaseArtifact({
    ...IDENTITY,
    commit: "a".repeat(40),
    docsRoot,
    outputDirectory: path.join(temporary, "release"),
    repository: "cavi-ai/mcp-eval",
  }), /manifest is inconsistent/u);
});

import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { chmod, mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import test from "node:test";

const ROOT = path.resolve(import.meta.dirname, "../..");

async function fixtureArchive(directory) {
  const contents = path.join(directory, "contents");
  await mkdir(contents, { recursive: true });
  await writeFile(path.join(contents, "mcpeval"), "#!/bin/sh\nprintf 'fixture mcpeval 0.1.0\\n'\n");
  await writeFile(path.join(contents, "mcpeval-demo"), "#!/bin/sh\nprintf 'fixture demo\\n'\n");
  await chmod(path.join(contents, "mcpeval"), 0o755);
  await chmod(path.join(contents, "mcpeval-demo"), 0o755);
  const archive = path.join(directory, "mcpeval-x86_64-unknown-linux-gnu.tar.gz");
  const packed = spawnSync("tar", ["-czf", archive, "-C", contents, "mcpeval", "mcpeval-demo"], {
    encoding: "utf8",
  });
  assert.equal(packed.status, 0, packed.stderr);
  const bytes = await readFile(archive);
  return {
    archive,
    bytes,
    sha256: createHash("sha256").update(bytes).digest("hex"),
  };
}

function releaseManifest(sha256, size) {
  return {
    schemaVersion: 1,
    package: "@cavi-ai/mcp-eval",
    repository: "cavi-ai/mcp-eval",
    version: "0.1.0",
    tag: "v0.1.0",
    commit: "ffc0b2d0d0922f3f4daa246effcfa25fe7be349a",
    assets: {
      "linux-x64": {
        target: "x86_64-unknown-linux-gnu",
        archive: "mcpeval-x86_64-unknown-linux-gnu.tar.gz",
        sha256,
        size,
      },
    },
  };
}

/** `failures` maps a path to the statuses it answers, in order, before serving its body. */
async function fixtureServer(routes, failures = new Map()) {
  const requests = [];
  const server = http.createServer((request, response) => {
    requests.push(request.url);
    const failure = failures.get(request.url)?.shift();
    if (failure !== undefined) {
      response.writeHead(failure).end();
      return;
    }
    const body = routes.get(request.url);
    if (body === undefined) {
      response.writeHead(404).end();
      return;
    }
    response.writeHead(200, { "content-type": "application/octet-stream" });
    response.end(body);
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = server.address();
  return {
    baseUrl: `http://127.0.0.1:${address.port}`,
    requests,
    close: () => new Promise((resolve, reject) => server.close((error) => error ? reject(error) : resolve())),
  };
}

test("installer verifies the pinned checksum before exposing the binary", async (context) => {
  const temporary = await mkdtemp(path.join(os.tmpdir(), "mcpeval-npm-install-"));
  context.after(() => rm(temporary, { recursive: true, force: true }));
  const fixture = await fixtureArchive(temporary);
  const assetName = path.basename(fixture.archive);
  const server = await fixtureServer(new Map([
    [`/${assetName}`, fixture.bytes],
    [`/${assetName}.sha256`, `${fixture.sha256}  ${assetName}\n`],
  ]));
  context.after(server.close);

  const { installRelease } = await import(path.join(ROOT, "npm/install.mjs"));
  const packageRoot = path.join(temporary, "package");
  await mkdir(path.join(packageRoot, "npm"), { recursive: true });
  await installRelease({
    manifest: releaseManifest(fixture.sha256, fixture.bytes.length),
    packageRoot,
    platform: "linux",
    arch: "x64",
    downloadBaseUrl: server.baseUrl,
  });

  const installed = path.join(packageRoot, "npm/vendor/mcpeval");
  const executed = spawnSync(installed, [], { encoding: "utf8" });
  assert.equal(executed.status, 0, executed.stderr);
  assert.equal(executed.stdout, "fixture mcpeval 0.1.0\n");
  const demo = spawnSync(path.join(packageRoot, "npm/vendor/mcpeval-demo"), [], { encoding: "utf8" });
  assert.equal(demo.status, 0, demo.stderr);
  assert.equal(demo.stdout, "fixture demo\n");
  assert.equal(await readFile(path.join(packageRoot, "npm/vendor/.version"), "utf8"), "v0.1.0 x86_64-unknown-linux-gnu\n");
});

test("installer rejects altered archive bytes even when the checksum filename is valid", async (context) => {
  const temporary = await mkdtemp(path.join(os.tmpdir(), "mcpeval-npm-tamper-"));
  context.after(() => rm(temporary, { recursive: true, force: true }));
  const fixture = await fixtureArchive(temporary);
  const assetName = path.basename(fixture.archive);
  const altered = Buffer.from(fixture.bytes);
  altered[Math.floor(altered.length / 2)] ^= 0xff;
  const server = await fixtureServer(new Map([
    [`/${assetName}`, altered],
    [`/${assetName}.sha256`, `${fixture.sha256}  ${assetName}\n`],
  ]));
  context.after(server.close);
  const { installRelease } = await import(path.join(ROOT, "npm/install.mjs"));
  const packageRoot = path.join(temporary, "package");

  await assert.rejects(
    installRelease({
      manifest: releaseManifest(fixture.sha256, fixture.bytes.length),
      packageRoot,
      platform: "linux",
      arch: "x64",
      downloadBaseUrl: server.baseUrl,
    }),
    /archive SHA-256 mismatch/u,
  );
  await assert.rejects(readFile(path.join(packageRoot, "npm/vendor/mcpeval")), { code: "ENOENT" });
});

test("installer rejects an archive larger than its pinned release size", async (context) => {
  const temporary = await mkdtemp(path.join(os.tmpdir(), "mcpeval-npm-oversize-"));
  context.after(() => rm(temporary, { recursive: true, force: true }));
  const fixture = await fixtureArchive(temporary);
  const assetName = path.basename(fixture.archive);
  const oversized = Buffer.concat([fixture.bytes, Buffer.from("unexpected")]);
  const server = await fixtureServer(new Map([
    [`/${assetName}`, oversized],
    [`/${assetName}.sha256`, `${fixture.sha256}  ${assetName}\n`],
  ]));
  context.after(server.close);
  const { installRelease } = await import(path.join(ROOT, "npm/install.mjs"));

  await assert.rejects(
    installRelease({
      manifest: releaseManifest(fixture.sha256, fixture.bytes.length),
      packageRoot: path.join(temporary, "package"),
      platform: "linux",
      arch: "x64",
      downloadBaseUrl: server.baseUrl,
    }),
    /download exceeds the pinned size/u,
  );
});

async function retryFixture(context, failures) {
  const temporary = await mkdtemp(path.join(os.tmpdir(), "mcpeval-npm-retry-"));
  context.after(() => rm(temporary, { recursive: true, force: true }));
  const fixture = await fixtureArchive(temporary);
  const assetName = path.basename(fixture.archive);
  const routes = new Map([
    [`/${assetName}`, fixture.bytes],
    [`/${assetName}.sha256`, `${fixture.sha256}  ${assetName}\n`],
  ]);
  const server = await fixtureServer(routes, failures(assetName));
  context.after(server.close);
  const { installRelease } = await import(path.join(ROOT, "npm/install.mjs"));
  const packageRoot = path.join(temporary, "package");
  const install = () => installRelease({
    manifest: releaseManifest(fixture.sha256, fixture.bytes.length),
    packageRoot,
    platform: "linux",
    arch: "x64",
    downloadBaseUrl: server.baseUrl,
    retryDelaysMs: [0, 0],
  });
  return { assetName, server, packageRoot, install };
}

test("installer retries transient server errors and then installs", async (context) => {
  const { assetName, server, packageRoot, install } = await retryFixture(context, (asset) => new Map([
    [`/${asset}.sha256`, [500]],
    [`/${asset}`, [503, 429]],
  ]));
  await install();

  const executed = spawnSync(path.join(packageRoot, "npm/vendor/mcpeval"), [], { encoding: "utf8" });
  assert.equal(executed.status, 0, executed.stderr);
  assert.equal(executed.stdout, "fixture mcpeval 0.1.0\n");
  assert.deepEqual(server.requests, [
    `/${assetName}.sha256`, `/${assetName}.sha256`,
    `/${assetName}`, `/${assetName}`, `/${assetName}`,
  ]);
});

test("installer does not retry a missing asset", async (context) => {
  const { assetName, server, install } = await retryFixture(context, (asset) => new Map([[`/${asset}.sha256`, [404]]]));
  await assert.rejects(install(), /download failed \(404\)/u);
  assert.deepEqual(server.requests, [`/${assetName}.sha256`]);
});

test("installer gives up after its last retry", async (context) => {
  const { server, packageRoot, install } = await retryFixture(context, (asset) => new Map([[`/${asset}.sha256`, [500, 500, 500]]]));
  await assert.rejects(install(), /download failed \(500\) for \S+ after 3 attempts/u);
  assert.equal(server.requests.length, 3);
  await assert.rejects(readFile(path.join(packageRoot, "npm/vendor/mcpeval")), { code: "ENOENT" });
});

test("installer rejects unsupported platform pairs before download", async () => {
  const { installRelease } = await import(path.join(ROOT, "npm/install.mjs"));
  await assert.rejects(
    installRelease({
      manifest: releaseManifest("0".repeat(64), 1),
      packageRoot: "/unused",
      platform: "freebsd",
      arch: "x64",
      downloadBaseUrl: "http://127.0.0.1:1",
    }),
    /unsupported platform freebsd-x64/u,
  );
});

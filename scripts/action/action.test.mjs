import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { existsSync } from "node:fs";
import { chmod, mkdir, mkdtemp, readFile, symlink, writeFile } from "node:fs/promises";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { spawn, spawnSync } from "node:child_process";
import test from "node:test";

const ROOT = path.resolve(import.meta.dirname, "../..");
const INSTALL = path.join(ROOT, "scripts/action/install.sh");
const RUN = path.join(ROOT, "scripts/action/run.sh");
const RELEASE_BIN = path.join(ROOT, "target/release");
const TAG = "v9.9.9";
const LINUX_ARCHIVE = "mcpeval-x86_64-unknown-linux-gnu.tar.gz";
const WINDOWS_ARCHIVE = "mcpeval-x86_64-pc-windows-msvc.zip";

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

// PATH without any directory that holds an `mcpeval`, plus a directory that
// links the jq the scripts need.
async function pathWithoutMcpeval(directory) {
  const jq = spawnSync("sh", ["-c", "command -v jq"], { encoding: "utf8" }).stdout.trim();
  assert.ok(jq, "jq must be installed");
  const tools = path.join(directory, "tools");
  await mkdir(tools, { recursive: true });
  await symlink(jq, path.join(tools, "jq"));
  const kept = process.env.PATH.split(path.delimiter).filter(
    (entry) => entry && !existsSync(path.join(entry, "mcpeval")),
  );
  return [tools, ...kept].join(path.delimiter);
}

async function stubArchive(directory, { version, windows = false }) {
  const contents = path.join(directory, `contents-${version}-${windows}`);
  await mkdir(contents, { recursive: true });
  const [binary, demo] = windows ? ["mcpeval.exe", "mcpeval-demo.exe"] : ["mcpeval", "mcpeval-demo"];
  await writeFile(path.join(contents, binary), `#!/bin/sh\nprintf 'mcpeval ${version}\\n'\n`);
  await writeFile(path.join(contents, demo), "#!/bin/sh\nexit 0\n");
  await chmod(path.join(contents, binary), 0o755);
  await chmod(path.join(contents, demo), 0o755);
  const archive = path.join(directory, `${version}-${windows ? WINDOWS_ARCHIVE : LINUX_ARCHIVE}`);
  const args = windows
    ? ["--format", "zip", "-cf", archive, "-C", contents, binary, demo]
    : ["-czf", archive, "-C", contents, binary, demo];
  const packed = spawnSync("tar", args, { encoding: "utf8" });
  assert.equal(packed.status, 0, packed.stderr);
  const bytes = await readFile(archive);
  return { bytes, sha256: sha256(bytes), size: bytes.length };
}

function manifestFor(tag, archiveName, key, stub) {
  return {
    schemaVersion: 1,
    package: "@cavi-ai/mcp-eval",
    repository: "cavi-ai/mcp-eval",
    version: tag.slice(1),
    tag,
    commit: "0".repeat(40),
    assets: {
      [key]: { target: "fixture", archive: archiveName, size: stub.size, sha256: stub.sha256 },
    },
  };
}

async function releaseServer(routes) {
  const requests = [];
  const server = http.createServer((request, response) => {
    requests.push(request.url);
    const body = routes.get(request.url);
    if (body === undefined) {
      response.writeHead(404).end();
      return;
    }
    response.writeHead(200, { "content-type": "application/octet-stream" });
    response.end(body);
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  return {
    baseUrl: `http://127.0.0.1:${server.address().port}`,
    requests,
    close: () => new Promise((resolve) => server.close(resolve)),
  };
}

function runScript(script, env, cwd) {
  return new Promise((resolve) => {
    const child = spawn("bash", [script], { cwd, env });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (chunk) => (stdout += chunk));
    child.stderr.on("data", (chunk) => (stderr += chunk));
    child.on("close", (status) => resolve({ status, stdout, stderr }));
  });
}

async function install({ manifest, routes, env = {}, pathValue }) {
  const directory = await mkdtemp(path.join(os.tmpdir(), "mcpeval-action-install-"));
  const actionPath = path.join(directory, "action");
  await mkdir(path.join(actionPath, "distribution"), { recursive: true });
  await writeFile(path.join(actionPath, "distribution/release.json"), JSON.stringify(manifest));
  const runnerTemp = path.join(directory, "runner-temp");
  await mkdir(runnerTemp);
  const githubPath = path.join(directory, "github-path");
  await writeFile(githubPath, "");
  const server = await releaseServer(routes);
  try {
    const result = await runScript(
      INSTALL,
      {
        PATH: pathValue ?? (await pathWithoutMcpeval(directory)),
        HOME: process.env.HOME,
        ACTION_PATH: actionPath,
        GITHUB_PATH: githubPath,
        RUNNER_TEMP: runnerTemp,
        RUNNER_OS: "Linux",
        RUNNER_ARCH: "X64",
        MCPEVAL_RELEASE_BASE_URL: server.baseUrl,
        ...env,
      },
      directory,
    );
    return { ...result, directory, requests: server.requests, githubPath: await readFile(githubPath, "utf8") };
  } finally {
    await server.close();
  }
}

function releaseRoutes(tag, archiveName, stub, companion = `${stub.sha256}  ${archiveName}\n`) {
  return new Map([
    [`/${tag}/${archiveName}`, stub.bytes],
    [`/${tag}/${archiveName}.sha256`, companion],
  ]);
}

test("install verifies the pinned archive and puts mcpeval and mcpeval-demo on GITHUB_PATH", async () => {
  const directory = await mkdtemp(path.join(os.tmpdir(), "mcpeval-action-stub-"));
  const stub = await stubArchive(directory, { version: "9.9.9" });
  const result = await install({
    manifest: manifestFor(TAG, LINUX_ARCHIVE, "linux-x64", stub),
    routes: releaseRoutes(TAG, LINUX_ARCHIVE, stub),
  });
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /^installed mcpeval 9\.9\.9 \(linux-x64\) from http:.*\/v9\.9\.9\/mcpeval-x86_64-unknown-linux-gnu\.tar\.gz$/mu);
  const entries = result.githubPath.split("\n").filter(Boolean);
  assert.equal(entries.length, 1);
  const installed = spawnSync(path.join(entries[0], "mcpeval"), ["--version"], { encoding: "utf8" });
  assert.equal(installed.stdout, "mcpeval 9.9.9\n");
  assert.ok(existsSync(path.join(entries[0], "mcpeval-demo")));
});

test("install rejects every mismatch with exit 1 and a one-line reason", async () => {
  const directory = await mkdtemp(path.join(os.tmpdir(), "mcpeval-action-stub-"));
  const stub = await stubArchive(directory, { version: "9.9.9" });
  const pinned = manifestFor(TAG, LINUX_ARCHIVE, "linux-x64", stub);
  const tampered = Buffer.from(stub.bytes);
  tampered[tampered.length - 1] ^= 0xff;
  const cases = [
    {
      name: "companion digest",
      routes: releaseRoutes(TAG, LINUX_ARCHIVE, stub, `${"0".repeat(64)}  ${LINUX_ARCHIVE}\n`),
      reason: `checksum companion does not match the pinned SHA-256 for ${LINUX_ARCHIVE}`,
    },
    {
      name: "companion format",
      routes: releaseRoutes(TAG, LINUX_ARCHIVE, stub, `${stub.sha256} ${LINUX_ARCHIVE}\n`),
      reason: `invalid checksum companion for ${LINUX_ARCHIVE}`,
    },
    {
      name: "companion file name",
      routes: releaseRoutes(TAG, LINUX_ARCHIVE, stub, `${stub.sha256}  other.tar.gz\n`),
      reason: `invalid checksum companion for ${LINUX_ARCHIVE}`,
    },
    {
      name: "manifest sha256",
      manifest: { ...pinned, assets: { "linux-x64": { ...pinned.assets["linux-x64"], sha256: "f".repeat(64) } } },
      reason: `checksum companion does not match the pinned SHA-256 for ${LINUX_ARCHIVE}`,
    },
    {
      name: "manifest size",
      manifest: { ...pinned, assets: { "linux-x64": { ...pinned.assets["linux-x64"], size: stub.size + 1 } } },
      reason: `size mismatch for ${LINUX_ARCHIVE}: expected ${stub.size + 1}, received ${stub.size}`,
    },
    {
      name: "archive bytes",
      routes: releaseRoutes(TAG, LINUX_ARCHIVE, { ...stub, bytes: tampered }),
      reason: `archive SHA-256 mismatch for ${LINUX_ARCHIVE}`,
    },
    {
      name: "missing asset",
      env: { RUNNER_ARCH: "ARM64" },
      reason: `release ${TAG} has no linux-arm64 asset`,
    },
    {
      name: "unsupported runner",
      env: { RUNNER_OS: "FreeBSD" },
      reason: "unsupported runner OS 'FreeBSD'",
    },
  ];
  for (const { name, manifest = pinned, routes = releaseRoutes(TAG, LINUX_ARCHIVE, stub), env, reason } of cases) {
    const result = await install({ manifest, routes, env });
    assert.equal(result.status, 1, `${name}: ${result.stderr}`);
    assert.equal(result.stderr, `mcpeval install: ${reason}\n`, name);
    assert.equal(result.githubPath, "", name);
  }
});

test("install short-circuits on a preinstalled mcpeval without downloading", async () => {
  const directory = await mkdtemp(path.join(os.tmpdir(), "mcpeval-action-stub-"));
  const preinstalled = path.join(directory, "preinstalled");
  await mkdir(preinstalled);
  await writeFile(path.join(preinstalled, "mcpeval"), "#!/bin/sh\nprintf 'mcpeval 1.2.3\\n'\n");
  await chmod(path.join(preinstalled, "mcpeval"), 0o755);
  const stub = await stubArchive(directory, { version: "9.9.9" });
  const result = await install({
    manifest: manifestFor(TAG, LINUX_ARCHIVE, "linux-x64", stub),
    routes: releaseRoutes(TAG, LINUX_ARCHIVE, stub),
    pathValue: [preinstalled, await pathWithoutMcpeval(directory)].join(path.delimiter),
  });
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout, "using preinstalled mcpeval 1.2.3\n");
  assert.deepEqual(result.requests, []);
  assert.equal(result.githubPath, "");
});

test("install with a version pins to that release's own release.json", async () => {
  const directory = await mkdtemp(path.join(os.tmpdir(), "mcpeval-action-stub-"));
  const pinnedStub = await stubArchive(directory, { version: "9.9.9" });
  const requestedStub = await stubArchive(directory, { version: "1.0.0" });
  const requestedManifest = manifestFor("v1.0.0", LINUX_ARCHIVE, "linux-x64", requestedStub);
  const routes = new Map([
    ...releaseRoutes("v1.0.0", LINUX_ARCHIVE, requestedStub),
    ["/v1.0.0/release.json", JSON.stringify(requestedManifest)],
  ]);
  const result = await install({
    manifest: manifestFor(TAG, LINUX_ARCHIVE, "linux-x64", pinnedStub),
    routes,
    env: { MCPEVAL_VERSION: "1.0.0" },
  });
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /^installed mcpeval 1\.0\.0 \(linux-x64\)/mu);
  assert.equal(result.requests[0], "/v1.0.0/release.json");

  const mislabeled = await install({
    manifest: manifestFor(TAG, LINUX_ARCHIVE, "linux-x64", pinnedStub),
    routes: new Map([...routes, ["/v1.0.0/release.json", JSON.stringify({ ...requestedManifest, tag: "v2.0.0" })]]),
    env: { MCPEVAL_VERSION: "v1.0.0" },
  });
  assert.equal(mislabeled.status, 1, mislabeled.stderr);
  assert.match(mislabeled.stderr, /^mcpeval install: http:.*\/v1\.0\.0\/release\.json does not describe v1\.0\.0\n$/u);
  assert.equal(mislabeled.githubPath, "");
});

const bsdtar = /bsdtar/u.test(spawnSync("tar", ["--version"], { encoding: "utf8" }).stdout ?? "");

test("install extracts the Windows zip archive", { skip: !bsdtar && "needs a tar that reads zip" }, async () => {
  const directory = await mkdtemp(path.join(os.tmpdir(), "mcpeval-action-stub-"));
  const stub = await stubArchive(directory, { version: "9.9.9", windows: true });
  const result = await install({
    manifest: manifestFor(TAG, WINDOWS_ARCHIVE, "win32-x64", stub),
    routes: releaseRoutes(TAG, WINDOWS_ARCHIVE, stub, `${stub.sha256}  ${WINDOWS_ARCHIVE}`),
    env: { RUNNER_OS: "Windows" },
  });
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /^installed mcpeval 9\.9\.9 \(win32-x64\)/mu);
  const [entry] = result.githubPath.split("\n");
  assert.ok(existsSync(path.join(entry, "mcpeval-demo.exe")));
});

const built = existsSync(path.join(RELEASE_BIN, "mcpeval")) && existsSync(path.join(RELEASE_BIN, "mcpeval-demo"));
const needsBuild = !built && "run `cargo build --release --locked` first";

async function actionWorkspace() {
  const directory = await mkdtemp(path.join(os.tmpdir(), "mcpeval-action-run-"));
  const pathValue = [RELEASE_BIN, await pathWithoutMcpeval(directory)].join(path.delimiter);
  const init = spawnSync(
    "mcpeval",
    ["init", "--server", "demo", "--confirm-read-only", "--output", "mcp-eval.manifest.json", "--", "mcpeval-demo"],
    { cwd: directory, encoding: "utf8", env: { ...process.env, PATH: pathValue } },
  );
  assert.equal(init.status, 0, init.stderr);
  return { directory, pathValue };
}

async function runAction(workspace, inputs, pathValue = workspace.pathValue) {
  const step = await mkdtemp(path.join(workspace.directory, "step-"));
  const output = path.join(step, "output");
  const summary = path.join(step, "summary");
  await writeFile(output, "");
  await writeFile(summary, "");
  const env = { PATH: pathValue, HOME: process.env.HOME, GITHUB_OUTPUT: output, GITHUB_STEP_SUMMARY: summary };
  for (const [name, value] of Object.entries(inputs)) {
    env[`INPUT_${name.toUpperCase().replaceAll("-", "_")}`] = value;
  }
  const result = await runScript(RUN, env, workspace.directory);
  const outputs = Object.fromEntries(
    (await readFile(output, "utf8"))
      .split("\n")
      .filter(Boolean)
      .map((line) => [line.slice(0, line.indexOf("=")), line.slice(line.indexOf("=") + 1)]),
  );
  return { ...result, outputs, summary: await readFile(summary, "utf8") };
}

test("run passes a clean server and writes outputs and the job summary", { skip: needsBuild }, async () => {
  const workspace = await actionWorkspace();
  const result = await runAction(workspace, { server: "demo", command: "mcpeval-demo" });
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(result.outputs, {
    report: "mcpeval.report.json",
    "exit-code": "0",
    readiness: "100",
    passed: "true",
    markdown: "mcpeval.report.md",
  });
  assert.match(result.summary, /^## mcp-eval report — demo$/mu);
  const report = JSON.parse(await readFile(path.join(workspace.directory, "mcpeval.report.json"), "utf8"));
  assert.equal(report.schema, "mcpeval.probe-report/v1");
  const markdown = await readFile(path.join(workspace.directory, result.outputs.markdown), "utf8");
  assert.match(markdown, /^## mcp-eval report — demo$/mu);
});

test("run fails a broken server with the probe's exit code", { skip: needsBuild }, async () => {
  const workspace = await actionWorkspace();
  const result = await runAction(workspace, { server: "demo", command: "mcpeval-demo --broken stalled-cursor" });
  assert.equal(result.status, 1, result.stderr);
  assert.equal(result.outputs.passed, "false");
  assert.equal(result.outputs["exit-code"], "1");
  assert.match(result.summary, /pagination-stalled-cursor/u);
});

test("run passes JSON-array command arguments intact and whitespace-split ones literally", { skip: needsBuild }, async () => {
  const workspace = await actionWorkspace();
  const wrapper = path.join(workspace.directory, "demo wrapper.sh");
  await writeFile(
    wrapper,
    '#!/bin/sh\n[ "$#" -eq 2 ] && [ "$1" = "two words" ] && [ "$2" = "*" ] || exit 97\nexec mcpeval-demo\n',
  );
  await chmod(wrapper, 0o755);
  const json = await runAction(workspace, { server: "demo", command: `  ${JSON.stringify([wrapper, "two words", "*"])}` });
  assert.equal(json.status, 0, json.stderr);
  assert.equal(json.outputs.passed, "true");

  const glob = path.join(workspace.directory, "glob.sh");
  await writeFile(glob, '#!/bin/sh\n[ "$#" -eq 1 ] && [ "$1" = "*" ] || exit 97\nexec mcpeval-demo\n');
  await chmod(glob, 0o755);
  const split = await runAction(workspace, { server: "demo", command: `${glob}\n  *` });
  assert.equal(split.status, 0, split.stderr);
  assert.equal(split.outputs.passed, "true");
});

test("run rejects invalid command and url combinations with exit 2", { skip: needsBuild }, async () => {
  const workspace = await actionWorkspace();
  const cases = [
    [{ server: "demo", command: "mcpeval-demo", url: "http://127.0.0.1:1/mcp" }, "set exactly one of the command and url inputs"],
    [{ server: "demo", command: "  " }, "set exactly one of the command and url inputs"],
    [{ server: "demo", command: '["mcpeval-demo", 1]' }, "command starts with '[' but is not a JSON array of strings"],
    [{ server: "demo", command: "[mcpeval-demo" }, "command starts with '[' but is not a JSON array of strings"],
  ];
  for (const [inputs, message] of cases) {
    const result = await runAction(workspace, inputs);
    assert.equal(result.status, 2, JSON.stringify(inputs));
    assert.equal(result.stderr, `::error::mcpeval action: ${message}\n`);
    assert.deepEqual(result.outputs, {});
  }
});

test("run reports a probe that could not start without a summary", { skip: needsBuild }, async () => {
  const workspace = await actionWorkspace();
  const result = await runAction(workspace, { server: "demo", command: "mcpeval-demo", manifest: "missing.json" });
  assert.equal(result.status, 2, result.stderr);
  assert.deepEqual(result.outputs, { report: "mcpeval.report.json", "exit-code": "2", passed: "false" });
  assert.equal(result.summary, "");
});

test("run gates on a baseline and fails with the diff when only the diff fires", { skip: needsBuild }, async () => {
  const workspace = await actionWorkspace();
  const baseline = await runAction(workspace, { server: "demo", command: "mcpeval-demo", "report-path": "reports/baseline.json" });
  assert.equal(baseline.status, 0, baseline.stderr);
  assert.equal(baseline.outputs.report, "reports/baseline.json");

  const regressed = await runAction(workspace, {
    server: "demo",
    command: "mcpeval-demo --broken stalled-cursor",
    baseline: "reports/baseline.json",
  });
  assert.equal(regressed.status, 1, regressed.stderr);
  assert.equal(regressed.outputs["diff-exit-code"], "1");
  assert.match(regressed.summary, /^## mcp-eval baseline diff$/mu);
  assert.match(regressed.summary, /\*\*regressed\*\* \(`pagination-stalled-cursor`\)/u);
  assert.equal(regressed.outputs.markdown, "mcpeval.report.md");
  const regressedMarkdown = await readFile(path.join(workspace.directory, regressed.outputs.markdown), "utf8");
  assert.match(regressedMarkdown, /^## mcp-eval report — demo$/mu);
  assert.match(regressedMarkdown, /^## mcp-eval baseline diff$/mu);

  const other = await runAction(workspace, { server: "other", command: "mcpeval-demo", "report-path": "reports/other.json" });
  assert.equal(other.status, 0, other.stderr);
  const mismatched = await runAction(workspace, { server: "demo", command: "mcpeval-demo", baseline: "reports/other.json" });
  assert.equal(mismatched.outputs["exit-code"], "0");
  assert.equal(mismatched.outputs["diff-exit-code"], "2");
  assert.equal(mismatched.status, 2, mismatched.stderr);
});

test("run renders SARIF located at the manifest when sarif is true", { skip: needsBuild }, async () => {
  const workspace = await actionWorkspace();
  const result = await runAction(workspace, { server: "demo", command: "mcpeval-demo --broken stalled-cursor", sarif: "true" });
  assert.equal(result.status, 1, result.stderr);
  assert.equal(result.outputs.sarif, "mcpeval.sarif");
  const sarif = JSON.parse(await readFile(path.join(workspace.directory, result.outputs.sarif), "utf8"));
  assert.ok(sarif.runs[0].results.length > 0);
  assert.equal(sarif.runs[0].results[0].locations[0].physicalLocation.artifactLocation.uri, "mcp-eval.manifest.json");
});

test("run forwards url, mutation, and fail-on-change flags", async () => {
  const directory = await mkdtemp(path.join(os.tmpdir(), "mcpeval-action-argv-"));
  const stubs = path.join(directory, "stubs");
  await mkdir(stubs);
  const log = path.join(directory, "argv.log");
  await writeFile(
    path.join(stubs, "mcpeval"),
    `#!/bin/sh\nprintf '%s\\n' "$*" >> '${log}'\n` +
      `[ "$1" = probe ] && printf '{"passed":true,"readiness":{"score":100}}'\nexit 0\n`,
  );
  await chmod(path.join(stubs, "mcpeval"), 0o755);
  await writeFile(path.join(directory, "baseline.json"), "{}");
  const workspace = { directory, pathValue: [stubs, await pathWithoutMcpeval(directory)].join(path.delimiter) };
  const result = await runAction(workspace, {
    server: "remote",
    manifest: "m.json",
    url: "https://example.test/mcp",
    "allow-mutation": "true",
    "allow-remote-http": "true",
    baseline: "baseline.json",
    "fail-on-change": "true",
  });
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual((await readFile(log, "utf8")).split("\n").filter(Boolean), [
    "diff --help",
    "probe --server remote --manifest m.json --format json --allow-mutation --url https://example.test/mcp --allow-remote-http",
    "report mcpeval.report.json --format markdown",
    "diff baseline.json mcpeval.report.json --format markdown --fail-on-regression --fail-on-change",
  ]);
});

test("run refuses a baseline when mcpeval does not support diff", async () => {
  const directory = await mkdtemp(path.join(os.tmpdir(), "mcpeval-action-nodiff-"));
  const stubs = path.join(directory, "stubs");
  await mkdir(stubs);
  await writeFile(
    path.join(stubs, "mcpeval"),
    "#!/bin/sh\n" +
      'if [ "$1" = --version ]; then printf \'mcpeval 0.2.0\\n\'; exit 0; fi\n' +
      'if [ "$1" = diff ]; then echo "error: unrecognized subcommand \'diff\'" >&2; exit 2; fi\n' +
      "exit 0\n",
  );
  await chmod(path.join(stubs, "mcpeval"), 0o755);
  const workspace = { directory, pathValue: [stubs, await pathWithoutMcpeval(directory)].join(path.delimiter) };
  const result = await runAction(workspace, { server: "demo", command: "mcpeval-demo", baseline: "baseline.json" });
  assert.equal(result.status, 2, result.stdout + result.stderr);
  assert.equal(
    result.stderr,
    "::error::mcpeval action: mcpeval 0.2.0 does not support baseline; set the version input to 0.3.0 or later\n",
  );
  assert.deepEqual(result.outputs, {});
});

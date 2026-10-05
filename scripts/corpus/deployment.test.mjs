import assert from "node:assert/strict";
import { chmod, copyFile, mkdir, mkdtemp, rm, symlink, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { deploymentDigest } from "./deployment.mjs";
import { commandFor, evaluateTarget, ROOT, runCommand } from "./contract.mjs";
import { collectCorpus } from "./collect.mjs";
import { verifyCorpus, driftedResults } from "./verify.mjs";

async function bundle(t) {
  const root = await mkdtemp(path.join(os.tmpdir(), "mcpeval-deployment-test-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  await mkdir(path.join(root, "deps"));
  await writeFile(path.join(root, "runtime"), "fixture runtime");
  await writeFile(path.join(root, "server.mjs"), "fixture server");
  await writeFile(path.join(root, "deps", "dependency.mjs"), "fixture dependency");
  return root;
}
const target = (deployment) => ({ server: "fixture", runtime: "npm", package: "fixture-package", version: "1.2.3", bin: "fixture", args: ["--fixture"], prerequisites: { environment: [], checks: [] }, deployment });

test("deployment digest binds file bytes, names, permissions, and internal links without binding its location", async (t) => {
  const root = await bundle(t);
  const initial = await deploymentDigest(root);
  assert.match(initial, /^[a-f0-9]{64}$/u);
  const other = await bundle(t);
  assert.equal(await deploymentDigest(other), initial);
  for (const file of ["runtime", "server.mjs", "deps/dependency.mjs"]) {
    const original = file === "runtime" ? "fixture runtime" : file === "server.mjs" ? "fixture server" : "fixture dependency";
    await writeFile(path.join(root, file), "changed");
    assert.notEqual(await deploymentDigest(root), initial);
    await writeFile(path.join(root, file), original);
  }
  await symlink("runtime", path.join(root, "runtime-link"));
  assert.notEqual(await deploymentDigest(root), initial);
  await rm(path.join(root, "runtime-link"));
  if (process.platform !== "win32") {
    await chmod(path.join(root, "runtime"), 0o700);
    assert.notEqual(await deploymentDigest(root), initial);
  }
  await writeFile(path.join(other, "extra"), "fixture");
  assert.notEqual(await deploymentDigest(other), initial);
});

test("deployment refuses escaping links and unsafe launch paths", async (t) => {
  const root = await bundle(t);
  const deployment = { root, sha256: await deploymentDigest(root), executable: "runtime", entrypoint: "server.mjs" };
  assert.deepEqual(commandFor(target(deployment)), [path.join(root, "runtime"), path.join(root, "server.mjs"), "--fixture"]);
  for (const change of [{ root: "relative" }, { sha256: "unknown" }, { executable: "../outside" }, { executable: "/outside" }, { entrypoint: "a/../../outside" }, { executable: "runtime\\outside" }]) {
    assert.throws(() => commandFor(target({ ...deployment, ...change })));
  }
  await symlink(process.execPath, path.join(root, "external-runtime"));
  await assert.rejects(deploymentDigest(root), /outside deployment/u);
});

test("changed deployment cannot launch and changes during execution discard the report", async (t) => {
  const root = await bundle(t);
  const deployment = { root, sha256: await deploymentDigest(root), executable: "runtime", entrypoint: "server.mjs" };
  let launches = 0;
  const context = { environment: {}, home: root, binary: "fixture-evaluator", execute: async () => {
    launches++;
    await writeFile(path.join(root, "runtime"), "changed during execution");
    return { code: 0, stdout: "private invalid report" };
  } };
  assert.deepEqual(await evaluateTarget(target(deployment), context), { status: "errored", reason: "deployment-mismatch" });
  assert.equal(launches, 1);
  assert.deepEqual(await evaluateTarget(target(deployment), context), { status: "errored", reason: "deployment-mismatch" });
  assert.equal(launches, 1);
});

test("health checks cannot change a locked deployment before the server launch", async (t) => {
  const root = await bundle(t);
  const deployment = { root, sha256: await deploymentDigest(root), executable: "runtime" };
  const value = target(deployment);
  value.prerequisites.checks.push({ name: "health", command: ["fixture-check"] });
  let launches = 0;
  const result = await evaluateTarget(value, { environment: {}, home: root, execute: async () => {
    launches++;
    await writeFile(path.join(root, "runtime"), "check mutation");
    return { code: 0, stdout: "" };
  } });
  assert.deepEqual(result, { status: "errored", reason: "deployment-mismatch" });
  assert.equal(launches, 1);
});

test("native bundled deployment collects and replays without a package-launch substitute", async (t) => {
  const root = await bundle(t);
  const binary = path.resolve(process.env.MCPEVAL_CORPUS_TEST_BINARY ?? path.join(ROOT, "target/release", process.platform === "win32" ? "mcpeval.exe" : "mcpeval"));
  const demo = path.join(path.dirname(binary), process.platform === "win32" ? "mcpeval-demo.exe" : "mcpeval-demo");
  await copyFile(demo, path.join(root, "runtime"));
  await chmod(path.join(root, "runtime"), 0o755);
  const value = target({ root, sha256: await deploymentDigest(root), executable: "runtime" });
  value.args = [];
  const dir = await mkdtemp(path.join(os.tmpdir(), "mcpeval-locked-collection-"));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const targetsPath = path.join(dir, "targets.json");
  await writeFile(targetsPath, JSON.stringify({ schema: "mcpeval.corpus-targets/v1", standard: "mcpeval-standard/2", targets: [value] }));
  const options = { targetsPath, binary, output: path.join(dir, "corpus.json"), minimum: 1, environment: process.env };
  const collection = await collectCorpus(options);
  assert.equal(collection.corpus.observations[0].provenance.deployment_sha256, value.deployment.sha256);
  assert.ok(collection.corpus.observations[0].calls.successful_calls > 0);
  assert.ok(!JSON.stringify(collection.corpus).includes(root));
  const replay = { ...options, corpusPath: options.output, reportsPath: collection.reports };
  assert.deepEqual(driftedResults((await verifyCorpus(replay)).results), []);
  await writeFile(path.join(root, "deps", "dependency.mjs"), "changed dependency");
  let scores = 0;
  const result = await verifyCorpus({ ...replay, execute: (command, settings) => {
    if (command[1] === "score") scores++;
    return runCommand(command, settings);
  } });
  assert.equal(scores, 0);
  assert.deepEqual(result.results.map(({ status, reason }) => ({ status, reason })), [{ status: "errored", reason: "deployment-mismatch" }]);
});

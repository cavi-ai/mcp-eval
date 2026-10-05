import assert from "node:assert/strict";
import { copyFile, chmod, mkdir, mkdtemp, readdir, rm, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { deploymentDigest } from "./deployment.mjs";
import { ROOT, evaluateTarget, runCommand, sha256, validateTargets, validateCorpus } from "./contract.mjs";
import { collectCorpus } from "./collect.mjs";
import { verifyCorpus, driftedResults } from "./verify.mjs";

const expected = "a".repeat(64);
const check = { name: "backend", command: ["fixture-state-check"], sha256: expected };
const target = (state_checks = [check]) => ({ server: "fixture", runtime: "npm", package: "fixture-package", version: "1.2.3", bin: "fixture", args: [], prerequisites: { environment: [], checks: [], state_checks } });
const document = (value) => ({ schema: "mcpeval.corpus-targets/v1", standard: "mcpeval-standard/2", targets: [value] });

test("state checks require unique names, bounded commands, and exact digests", () => {
  for (const value of [null, {}, [{ ...check, sha256: "latest" }], [{ ...check, command: [] }], [{ ...check, command: ["a\0b"] }], [check, check], [{ ...check, name: "../private" }]]) {
    assert.throws(() => validateTargets(document(target(value))));
  }
  const value = target();
  value.prerequisites.checks = [{ name: check.name, command: ["health"] }];
  assert.throws(() => validateTargets(document(value)));
  assert.equal(validateTargets(document(target([]))).targets.length, 1);
});

test("failed, malformed, and mismatched state checks cannot launch an evaluator", async () => {
  for (const response of [{ code: 1, stdout: expected }, { code: 0, stdout: "PRIVATE_ERROR" }, { code: 0, stdout: expected, reason: "timeout" }, { code: 0, stdout: "b".repeat(64) }]) {
    const launches = [];
    const result = await evaluateTarget(target(), { environment: {}, home: os.tmpdir(), execute: async (command, options) => {
      launches.push({ command, options }); return response;
    } });
    assert.equal(launches.length, 1);
    assert.deepEqual(launches[0].command, check.command);
    assert.equal(launches[0].options.timeoutMs, 10_000);
    assert.equal(launches[0].options.maxBytes, 1024);
    assert.deepEqual(result, response.stdout === "b".repeat(64) ? { status: "errored", reason: "state-mismatch" } : { status: "untested", reason: "state-check-failed" });
    assert.ok(!JSON.stringify(result).includes("PRIVATE_ERROR"));
  }
});

test("post-evaluation state failure discards output even if evaluation already failed", async () => {
  for (const response of [{ code: 0, stdout: "b".repeat(64) }, { code: 1, stdout: "PRIVATE_STATE_FAILURE" }]) {
    let reads = 0;
    const result = await evaluateTarget(target(), { environment: {}, home: os.tmpdir(), binary: "fixture-evaluator", execute: async (command) => {
      if (command[0] === check.command[0]) return ++reads === 1 ? { code: 0, stdout: `${expected}\n` } : response;
      return { code: 3, stdout: "PRIVATE_REPORT" };
    } });
    assert.equal(reads, 2);
    assert.deepEqual(result, { status: "errored", reason: response.code === 0 ? "state-mismatch" : "state-check-failed" });
  }
});

test("native MCP collection and replay bind the state of the service actually read by tools", async (t) => {
  const dir = await mkdtemp(path.join(os.tmpdir(), "mcpeval-service-state-"));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const root = path.join(dir, "deployment");
  await mkdir(root);
  const runtime = process.platform === "win32" ? "node.exe" : "node";
  await copyFile(process.execPath, path.join(root, runtime));
  await chmod(path.join(root, runtime), 0o755);
  for (const file of ["state-check.mjs", "state-server.mjs"]) await copyFile(path.join(ROOT, "scripts/corpus/fixtures", file), path.join(root, file));
  const initial = JSON.stringify({ revision: "fixture/v1", records: ["PRIVATE_BACKEND_RECORD"] });
  let state = initial; let toolReads = 0; let mutate = false;
  const service = createServer((request, response) => {
    response.writeHead(200, { "Content-Type": "application/json" });
    response.end(state);
    if (request.headers["x-fixture-tool"]) {
      toolReads++;
      if (mutate) state = JSON.stringify({ revision: "fixture/v2", records: [] });
    }
  });
  await new Promise((resolve, reject) => { service.once("error", reject); service.listen(0, "127.0.0.1", resolve); });
  t.after(() => new Promise((resolve) => service.close(resolve)));
  const url = `http://127.0.0.1:${service.address().port}/state`;
  const value = target([{ name: "backend", command: [path.join(root, runtime), path.join(root, "state-check.mjs")], sha256: sha256(initial) }]);
  value.prerequisites.environment = ["FIXTURE_BACKEND_URL"];
  value.deployment = { root, executable: runtime, entrypoint: "state-server.mjs", sha256: await deploymentDigest(root) };
  const targetsPath = path.join(dir, "targets.json");
  await writeFile(targetsPath, JSON.stringify(document(value)));
  const binary = path.resolve(process.env.MCPEVAL_CORPUS_TEST_BINARY ?? path.join(ROOT, "target/release", process.platform === "win32" ? "mcpeval.exe" : "mcpeval"));
  const options = { binary, targetsPath, output: path.join(dir, "corpus.json"), minimum: 1, environment: { ...process.env, FIXTURE_BACKEND_URL: url } };
  const collection = await collectCorpus(options);
  assert.equal(collection.corpus.observations.length, 1);
  assert.ok(toolReads > 0);
  assert.ok(collection.corpus.observations[0].calls.successful_calls > 0);
  assert.deepEqual(collection.corpus.observations[0].provenance.state_checks, [{ name: "backend", sha256: sha256(initial) }]);
  assert.ok(!JSON.stringify(collection.corpus).includes("PRIVATE_BACKEND_RECORD"));
  const replay = { ...options, corpusPath: options.output, reportsPath: collection.reports };
  assert.deepEqual(driftedResults((await verifyCorpus(replay)).results), []);
  state = JSON.stringify({ revision: "fixture/v2", records: [] });
  const before = toolReads;
  const mismatch = await verifyCorpus(replay);
  assert.equal(toolReads, before);
  assert.equal(mismatch.results[0].reason, "state-mismatch");
  state = initial;
  mutate = true;
  const changed = await collectCorpus({ ...options, force: true });
  assert.equal(changed.sufficient, false);
  assert.deepEqual(changed.corpus.population, [{ server: "fixture", status: "errored", reason: "state-mismatch" }]);
  assert.deepEqual(changed.corpus.observations, []);
  assert.deepEqual(await readdir(changed.reports), []);
  assert.ok(toolReads > before);
  // Original projection and state identities must agree before any replay launch.
  const original = collection.corpus;
  original.observations[0].provenance.state_checks[0].sha256 = "0".repeat(64);
  await writeFile(options.output, JSON.stringify(original));
  let launches = 0;
  await assert.rejects(verifyCorpus({ ...replay, execute: (command, settings) => {
    if (command[1] === "score" || command.includes(path.join(root, "state-check.mjs"))) launches++;
    return runCommand(command, settings);
  } }), /projection mismatch/u);
  assert.equal(launches, 0);
  original.observations[0].provenance.state_checks.push({ name: "backend", sha256: expected });
  assert.throws(() => validateCorpus(original));
});

import assert from "node:assert/strict";
import test from "node:test";
import { readFile, writeFile } from "node:fs/promises";
import { fixture, target, report } from "./fixtures/evidence-fixture.mjs";
import { collectCorpus } from "./collect.mjs";
import { verifyCorpus } from "./verify.mjs";
import { validateTargets, observationFrom, sha256 } from "./contract.mjs";

const profile = (attested_read_only = false, skip_tools = []) => ({ schema: "mcpeval.measurement-profile/v1", evaluator_version: "0.4.0", platform: "linux", architecture: "x86_64", client_capabilities: "none", attested_read_only, skip_tools, call_timeout_ms: 15000, repeats: 3 });

test("reviewed assessment flags are launched, retained, and checked during replay", async (t) => {
  const assessment = { attest_read_only: true, skip_tools: ["write_query"] };
  let actual = profile(true, ["write_query"]);
  const f = await fixture(t, [target("one", { assessment })], server => report(server, { measurement_profile: actual }));
  const collection = await collectCorpus(f.options);
  const command = f.launches.find(x => x.command[1] === "score").command;
  assert.ok(command.includes("--confirm-read-only"));
  assert.ok(command.includes("--skip-tool"));
  assert.deepEqual(collection.corpus.observations[0].measurement_profile, actual);
  const options = { ...f.options, corpusPath: f.options.output, reportsPath: collection.reports };
  assert.equal((await verifyCorpus(options)).passed, true);
  actual = profile(false, ["write_query"]);
  const changed = await verifyCorpus(options);
  assert.equal(changed.passed, false);
  assert.equal(changed.results[0].reason, "invalid-report");
  actual = { ...profile(true, ["write_query"]), architecture: "aarch64" };
  const drift = await verifyCorpus(options);
  assert.equal(drift.passed, false);
  assert.deepEqual(drift.results[0].moved, ["measurement-profile"]);
  const { ROOT } = await import("./contract.mjs");
  const schema = JSON.parse(await readFile(`${ROOT}/docs/mcp-eval.corpus-verification.schema.json`));
  assert.match(drift.results[0].moved[0], new RegExp(schema.$defs.result.properties.moved.items.pattern));
});

test("assessment must be explicit and tool names bounded, unique, and safe", () => {
  const document = assessment => ({ schema: "mcpeval.corpus-targets/v1", standard: "mcpeval-standard/2", targets: [target("one", { assessment })] });
  for (const invalid of [null, {}, { attest_read_only: "yes", skip_tools: [] }, { attest_read_only: true }, { attest_read_only: true, skip_tools: ["duplicate", "duplicate"] }, { attest_read_only: true, skip_tools: ["https://private.invalid"] }, { attest_read_only: false, skip_tools: [], arbitrary: true }]) assert.throws(() => validateTargets(document(invalid)));
  assert.equal(validateTargets(document({ attest_read_only: false, skip_tools: [] })).targets.length, 1);
});

test("old v3 reports remain replayable without inventing stored profiles", async (t) => {
  const f = await fixture(t, [target("one")], server => report(server, { measurement_profile: profile() }));
  const collection = await collectCorpus(f.options);
  const old = JSON.parse(await readFile(f.options.output));
  delete old.observations[0].measurement_profile;
  await writeFile(f.options.output, JSON.stringify(old));
  assert.equal((await verifyCorpus({ ...f.options, corpusPath: f.options.output, reportsPath: collection.reports })).passed, true);
});

test("declared assessment cannot be credited from a report lacking its profile", () => {
  const value = target("one", { assessment: { attest_read_only: true, skip_tools: [] } });
  assert.throws(() => observationFrom(JSON.stringify(report("one")), value, { version: "0.4.0", sha256: sha256("fixture") }, "mcpeval-standard/2"));
});

test("strict release collection and replay reject unresolved runners before execution", async (t) => {
  const f = await fixture(t, [target("one")]);
  await assert.rejects(collectCorpus({ ...f.options, requireDeployments: true }), /deployment/);
  assert.equal(f.launches.length, 0);
  const collection = await collectCorpus(f.options);
  f.launches.length = 0;
  await assert.rejects(verifyCorpus({ ...f.options, corpusPath: f.options.output, reportsPath: collection.reports, requireDeployments: true }), /deployment/);
  assert.equal(f.launches.length, 0);
});

test("native reviewed assessment calls legacy readers but never explicit or skipped writers", async (t) => {
  const { copyFile, mkdir, mkdtemp, rm, access } = await import("node:fs/promises");
  const os = await import("node:os"); const path = await import("node:path");
  const { deploymentDigest } = await import("./deployment.mjs");
  const { ROOT } = await import("./contract.mjs");
  const { checkEvidence } = await import("./check-evidence.mjs");
  const dir = await mkdtemp(path.join(os.tmpdir(), "mcpeval-reviewed-policy-"));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const root = path.join(dir, "deployment"); await mkdir(root);
  const runtime = process.platform === "win32" ? "node.exe" : "node";
  await copyFile(process.execPath, path.join(root, runtime));
  await copyFile(path.join(ROOT, "scripts/corpus/fixtures/reviewed-policy-server.mjs"), path.join(root, "server.mjs"));
  const marker = path.join(dir, "writer-marker");
  const value = target("reviewed", { assessment: { attest_read_only: true, skip_tools: ["unreviewed_write"] },
    deployment: { root, executable: runtime, entrypoint: "server.mjs", sha256: await deploymentDigest(root) },
    prerequisites: { environment: ["FIXTURE_MUTATION_MARKER"], checks: [] } });
  const targetsPath = path.join(dir, "targets.json");
  await writeFile(targetsPath, JSON.stringify({ schema: "mcpeval.corpus-targets/v1", standard: "mcpeval-standard/2", targets: [value] }));
  const binary = process.env.MCPEVAL_CORPUS_TEST_BINARY ?? path.join(ROOT, "target/release", process.platform === "win32" ? "mcpeval.exe" : "mcpeval");
  const options = { targetsPath, binary, output: path.join(dir, "corpus.json"), minimum: 1, requireDeployments: true, environment: { PATH: process.env.PATH, FIXTURE_MUTATION_MARKER: marker } };
  const collection = await collectCorpus(options);
  assert.equal(collection.corpus.observations.length, 1);
  assert.ok(collection.corpus.observations[0].calls.successful_calls > 0);
  assert.equal(collection.corpus.observations[0].measurement_profile.attested_read_only, true);
  const evidencePath = path.join(dir, "evidence");
  const replay = { ...options, corpusPath: options.output, reportsPath: collection.reports, evidencePath };
  assert.equal((await verifyCorpus(replay)).passed, true);
  assert.equal((await checkEvidence(replay)).passed, true);
  await assert.rejects(access(marker), { code: "ENOENT" });
});

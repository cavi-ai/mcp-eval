import assert from "node:assert/strict";
import { mkdir, mkdtemp, readFile, readdir, stat, symlink, writeFile, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { collectCorpus } from "./collect.mjs";
import { checkEvidence } from "./check-evidence.mjs";
import { verifyCorpus, driftOf, driftedResults, platformMismatch } from "./verify.mjs";
import { AREAS, COUNTS, ROOT, commandFor, validateTargets, runCommand, sha256, environmentFor } from "./contract.mjs";

const target = (server, extra = {}) => ({ server, runtime: "npm", package: "fixture-package", version: "1.2.3", bin: "fixture-server", args: [], prerequisites: { environment: [], checks: [] }, ...extra });
const report = (server, changes = {}) => ({ schema: "mcpeval.probe-report/v2", server, generator: { name: "mcpeval", version: "0.4.0" }, gate: null, cases: [], passed: true, readiness: {
  standard: "mcpeval-standard/2", score: 50, surface: { tools: 1, read_only: 1, writers: 0, exercised: 0 },
  areas: AREAS.map((name) => ({ name, score: 50, measurements: name === "reliability" ? Object.fromEntries(COUNTS.map((key) => [key, key === "tool_errors" ? 3 : 0])) : name === "context" ? { catalog_tokens: 100 } : {} })), ...changes,
} });

async function fixture(t, targets, evaluate = (server) => report(server)) {
  const dir = await mkdtemp(path.join(os.tmpdir(), "mcpeval-corpus-test-"));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const binary = path.join(dir, "evaluator"); await writeFile(binary, "fixture executable identity");
  const targetsPath = path.join(dir, "targets.json");
  await writeFile(targetsPath, JSON.stringify({ schema: "mcpeval.corpus-targets/v1", standard: "mcpeval-standard/2", targets }));
  const launches = [];
  const execute = async (command, options) => {
    launches.push({ command, options });
    if (command[1] === "--version") return { code: 0, stdout: "mcpeval 0.4.0\n" };
    if (command[0] !== binary) return { code: 1, stdout: "PRIVATE_PREREQUISITE_OUTPUT" };
    const value = await evaluate(command[3]);
    return typeof value === "string" ? { code: 3, stdout: value } : { code: 0, stdout: `${JSON.stringify(value)}\n` };
  };
  const options = { targetsPath, binary, output: path.join(dir, "corpus.json"), minimum: 1, execute, environment: { PATH: process.env.PATH, PRIVATE_UNDECLARED: "SECRET" } };
  return { dir, options, launches };
}

test("collector keeps declared population and distinguishes real failing calls from untested targets", async (t) => {
  const targets = [target("failing"), target("missing", { prerequisites: { environment: ["FIXTURE_TOKEN"], checks: [] } }), target("service", { prerequisites: { environment: [], checks: [{ name: "backend", command: [process.execPath, "health.mjs"] }] } }), target("no-calls"), target("broken")];
  const f = await fixture(t, targets, (server) => {
    if (server === "broken") return "PRIVATE_INVALID_REPORT";
    const result = report(server);
    if (server === "no-calls") result.readiness.areas.find((area) => area.name === "reliability").measurements.tool_errors = 0;
    return result;
  });
  const { corpus, reports, sufficient } = await collectCorpus(f.options);
  assert.equal(sufficient, true);
  assert.deepEqual(corpus.population.map((item) => [item.server, item.status, item.reason]), [["failing", "observed", undefined], ["missing", "untested", "missing-environment"], ["service", "untested", "prerequisite-failed"], ["no-calls", "untested", "no-tool-calls"], ["broken", "errored", "evaluation-failed"]]);
  assert.equal(corpus.observations.length, 1);
  assert.equal(corpus.observations[0].calls.tool_errors, 3);
  assert.equal(corpus.observations[0].score, 50);
  assert.match(corpus.evaluator.sha256, /^[a-f0-9]{64}$/u);
  assert.equal(corpus.observations[0].provenance.report_sha256, sha256(await readFile(path.join(reports, "failing.json"))));
  const text = await readFile(f.options.output, "utf8");
  assert.ok(!text.includes("PRIVATE_"));
  assert.ok(!f.launches.some(({ command }) => command.includes("missing") || command.includes("service")));
  const withPrivateMetadata = JSON.parse(text);
  withPrivateMetadata.population[1].note = "PRIVATE_UNOBSERVED_METADATA";
  await writeFile(f.options.output, JSON.stringify(withPrivateMetadata));
  const replay = await verifyCorpus({ ...f.options, corpusPath: f.options.output, reportsPath: reports });
  assert.equal(driftedResults(replay.results).length, 4);
  assert.equal(replay.passed, false);
  assert.ok(!JSON.stringify(replay).includes("PRIVATE_"));
});

test("original artifacts and projected scores must agree before drift replay", async (t) => {
  const f = await fixture(t, [target("one")]);
  const collection = await collectCorpus(f.options);
  const options = { ...f.options, corpusPath: f.options.output, reportsPath: collection.reports };
  const result = await verifyCorpus(options);
  assert.deepEqual(driftedResults(result.results), []);
  const original = await readFile(f.options.output, "utf8");
  for (const corrupt of [
    (doc) => { doc.evaluator.sha256 = "0".repeat(64); },
    (doc) => { doc.targets_sha256 = "0".repeat(64); },
    (doc) => { doc.observations[0].score = 60; },
    (doc) => { doc.observations[0].provenance.report_sha256 = "0".repeat(64); },
    (doc) => { doc.population.push({ server: "fake", status: "observed" }); },
  ]) {
    const doc = JSON.parse(original); corrupt(doc); await writeFile(f.options.output, JSON.stringify(doc));
    const before = f.launches.filter(({ command }) => command[1] === "score").length;
    await assert.rejects(verifyCorpus(options));
    assert.equal(f.launches.filter(({ command }) => command[1] === "score").length, before);
  }
  await writeFile(f.options.output, original);
  await writeFile(path.join(collection.reports, "one.json"), "changed original artifact");
  await assert.rejects(verifyCorpus(options), /digest mismatch/u);
});

test("replay artifacts bind original bytes, evaluator, policy, and individual report identities", async (t) => {
  let catalog = 50;
  const f = await fixture(t, [target("one")], (server) => {
    const value = report(server);
    value.readiness.areas.find((area) => area.name === "catalog").score = catalog;
    return value;
  });
  const collection = await collectCorpus(f.options);
  const options = { ...f.options, corpusPath: f.options.output, reportsPath: collection.reports };
  const accepted = await verifyCorpus(options);
  assert.equal(accepted.schema, "mcpeval.corpus-verification/v1");
  assert.equal(accepted.corpus_sha256, sha256(await readFile(f.options.output)));
  assert.equal(accepted.targets_sha256, sha256(await readFile(f.options.targetsPath)));
  assert.deepEqual(accepted.evaluator, collection.corpus.evaluator);
  assert.equal(accepted.standard, collection.corpus.standard);
  assert.equal(accepted.platform, process.platform);
  assert.deepEqual(accepted.policy, { reliability_tolerance: 10 });
  assert.equal(accepted.passed, true);
  assert.equal(accepted.results[0].expected_report_sha256, collection.corpus.observations[0].provenance.report_sha256);
  assert.equal(accepted.results[0].replay_report_sha256, accepted.results[0].expected_report_sha256);
  assert.ok(!JSON.stringify(accepted).includes(f.dir));
  catalog = 51;
  const drifted = await verifyCorpus(options);
  assert.equal(drifted.passed, false);
  assert.deepEqual(drifted.results[0].moved, ["catalog 50→51"]);
  assert.equal(drifted.results[0].expected_report_sha256, accepted.results[0].expected_report_sha256);
  assert.notEqual(drifted.results[0].replay_report_sha256, accepted.results[0].replay_report_sha256);
});

test("historical corpus refusal happens before any package or evaluator execution", async () => {
  let launched = false;
  await assert.rejects(verifyCorpus({ execute: () => { launched = true; throw new Error("must not launch"); } }), /historical/u);
  assert.equal(launched, false);
});

test("replay evidence retains exact reports and both passing and drifted verdicts privately", async (t) => {
  let catalog = 50;
  const f = await fixture(t, [target("one")], (server) => {
    const value = report(server);
    value.readiness.areas.find((area) => area.name === "catalog").score = catalog;
    return value;
  });
  const collection = await collectCorpus(f.options);
  const options = { ...f.options, corpusPath: f.options.output, reportsPath: collection.reports };
  for (const [label, score, passed] of [["passing", 50, true], ["drifted", 51, false]]) {
    catalog = score;
    const evidencePath = path.join(f.dir, label);
    const result = await verifyCorpus({ ...options, evidencePath });
    assert.equal(result.passed, passed);
    assert.deepEqual(JSON.parse(await readFile(path.join(evidencePath, "verification.json"))), result);
    const raw = await readFile(path.join(evidencePath, "reports/one.json"));
    assert.equal(raw.toString(), `${JSON.stringify(report("one", { areas: report("one").readiness.areas.map((area) => area.name === "catalog" ? { ...area, score } : area) }))}\n`);
    assert.equal(sha256(raw), result.results[0].replay_report_sha256);
    assert.deepEqual(await readdir(evidencePath), ["reports", "verification.json"]);
    assert.ok(!JSON.stringify(result).includes(f.dir));
    if (process.platform !== "win32") {
      assert.equal((await stat(evidencePath)).mode & 0o077, 0);
      assert.equal((await stat(path.join(evidencePath, "reports"))).mode & 0o077, 0);
      for (const file of ["verification.json", "reports/one.json"]) assert.equal((await stat(path.join(evidencePath, file))).mode & 0o077, 0);
    }
  }
});

test("existing replay outputs and concurrent writers are never replaced or launched over", async (t) => {
  const f = await fixture(t, [target("one")]);
  const collection = await collectCorpus(f.options);
  const evidencePath = path.join(f.dir, "evidence");
  await mkdir(evidencePath);
  await writeFile(path.join(evidencePath, "verification.json"), "EXISTING_EVIDENCE");
  const options = { ...f.options, corpusPath: f.options.output, reportsPath: collection.reports, evidencePath };
  const before = f.launches.length;
  await assert.rejects(verifyCorpus(options), { code: "EEXIST" });
  assert.equal(f.launches.length, before);
  assert.equal(await readFile(path.join(evidencePath, "verification.json"), "utf8"), "EXISTING_EVIDENCE");
  await rm(evidencePath, { recursive: true });
  const results = await Promise.allSettled([verifyCorpus(options), verifyCorpus(options)]);
  assert.equal(results.filter((result) => result.status === "fulfilled").length, 1);
  assert.equal(results.find((result) => result.status === "rejected").reason.code, "EEXIST");
  const saved = JSON.parse(await readFile(path.join(evidencePath, "verification.json")));
  assert.equal(saved.passed, true);
  assert.equal(sha256(await readFile(path.join(evidencePath, "reports/one.json"))), saved.results[0].replay_report_sha256);
});

test("fatal replay errors remove incomplete evidence, but unavailable report verdicts are retained", async (t) => {
  const f = await fixture(t, [target("one")]);
  const collection = await collectCorpus(f.options);
  const evidencePath = path.join(f.dir, "evidence");
  const options = { ...f.options, corpusPath: f.options.output, reportsPath: collection.reports, evidencePath };
  const execute = async (command, settings) => {
    const result = await f.options.execute(command, settings);
    if (command[1] === "score") {
      await writeFile(f.options.binary, "changed evaluator");
      assert.ok((await stat(evidencePath)).isDirectory());
      await assert.rejects(stat(path.join(evidencePath, "verification.json")), { code: "ENOENT" });
    }
    return result;
  };
  await assert.rejects(verifyCorpus({ ...options, execute }), /evaluator changed/u);
  await assert.rejects(stat(evidencePath), { code: "ENOENT" });
  await writeFile(f.options.binary, "fixture executable identity");
  const unavailable = await verifyCorpus({ ...options, execute: async (command, settings) => command[1] === "score" ? { code: 3, stdout: "PRIVATE_INVALID_REPORT" } : f.options.execute(command, settings) });
  assert.equal(unavailable.passed, false);
  assert.equal(unavailable.results[0].replay_report_sha256, null);
  assert.deepEqual(await readdir(path.join(evidencePath, "reports")), []);
  assert.deepEqual(JSON.parse(await readFile(path.join(evidencePath, "verification.json"))), unavailable);
});

test("report write failures remove the reserved bundle without a verification marker", async (t) => {
  const f = await fixture(t, [target("one")]);
  const collection = await collectCorpus(f.options);
  const evidencePath = path.join(f.dir, "evidence");
  const options = { ...f.options, corpusPath: f.options.output, reportsPath: collection.reports, evidencePath };
  const execute = async (command, settings) => {
    if (command[1] === "score") await mkdir(path.join(evidencePath, "reports/one.json"));
    return f.options.execute(command, settings);
  };
  await assert.rejects(verifyCorpus({ ...options, execute }), { code: "EEXIST" });
  await assert.rejects(stat(evidencePath), { code: "ENOENT" });
  await writeFile(path.join(collection.reports, "one.json"), "tampered original report");
  await assert.rejects(verifyCorpus(options), /original report digest mismatch/u);
  await assert.rejects(stat(evidencePath), { code: "ENOENT" });
});

test("valid unobserved replay reports are retained while original untested targets have no reports", async (t) => {
  let unobserved = false;
  const f = await fixture(t, [target("one"), target("missing", { prerequisites: { environment: ["FIXTURE_TOKEN"], checks: [] } })], (server) => {
    const value = report(server);
    if (unobserved) value.readiness.areas.find((area) => area.name === "reliability").measurements.tool_errors = 0;
    return value;
  });
  const collection = await collectCorpus(f.options);
  unobserved = true;
  const evidencePath = path.join(f.dir, "evidence");
  const result = await verifyCorpus({ ...f.options, corpusPath: f.options.output, reportsPath: collection.reports, evidencePath });
  assert.equal(result.passed, false);
  assert.equal(result.results[0].reason, "no-tool-calls");
  assert.equal(sha256(await readFile(path.join(evidencePath, "reports/one.json"))), result.results[0].replay_report_sha256);
  assert.equal(result.results[1].replay_report_sha256, null);
  assert.deepEqual(await readdir(path.join(evidencePath, "reports")), ["one.json"]);
  assert.deepEqual(await checkEvidence({ ...f.options, corpusPath: f.options.output, reportsPath: collection.reports, evidencePath }), result);
});

test("offline evidence CLI reconstructs passing and drifted verdicts without the evaluator", async (t) => {
  let catalog = 50;
  const f = await fixture(t, [target("one")], (server) => {
    const value = report(server);
    value.readiness.areas.find((area) => area.name === "catalog").score = catalog;
    return value;
  });
  const collection = await collectCorpus(f.options);
  const options = { ...f.options, corpusPath: f.options.output, reportsPath: collection.reports };
  const receipts = [];
  for (const [label, score] of [["passing", 50], ["drifted", 51]]) {
    catalog = score;
    const evidencePath = path.join(f.dir, label);
    receipts.push({ evidencePath, receipt: await verifyCorpus({ ...options, evidencePath }) });
  }
  await rm(f.options.binary);
  for (const { evidencePath, receipt } of receipts) {
    const cli = await runCommand([process.execPath, path.join(ROOT, "scripts/corpus/check-evidence.mjs"), "--corpus", options.corpusPath, "--targets", options.targetsPath, "--reports", options.reportsPath, "--evidence", evidencePath, "--json"]);
    assert.equal(cli.code, receipt.passed ? 0 : 1, cli.stdout);
    assert.deepEqual(JSON.parse(cli.stdout), receipt);
    assert.ok(!cli.stdout.includes(f.dir));
  }
});

test("offline checking rejects changed identities, policies, summaries, and incomplete report sets", async (t) => {
  const f = await fixture(t, [target("one")]);
  const collection = await collectCorpus(f.options);
  const options = { ...f.options, corpusPath: f.options.output, reportsPath: collection.reports, evidencePath: path.join(f.dir, "evidence") };
  const receipt = await verifyCorpus(options);
  const marker = path.join(options.evidencePath, "verification.json");
  for (const corrupt of [
    (doc) => { doc.schema = "mcpeval.corpus-verification/v99"; },
    (doc) => { doc.corpus_sha256 = "0".repeat(64); },
    (doc) => { doc.targets_sha256 = "0".repeat(64); },
    (doc) => { doc.evaluator.version = "different"; },
    (doc) => { doc.evaluator.sha256 = "0".repeat(64); },
    (doc) => { doc.standard = "mcpeval-standard/99"; },
    (doc) => { doc.platform = "another-platform"; },
    (doc) => { doc.policy.reliability_tolerance = 11; },
    (doc) => { doc.passed = false; },
    (doc) => { doc.observations = 0; },
    (doc) => { doc.population = 2; },
    (doc) => { doc.results = []; },
    (doc) => { doc.results.push(doc.results[0]); },
    (doc) => { doc.results[0].server = "../outside"; },
    (doc) => { doc.results[0].expected = 49; },
    (doc) => { doc.results[0].observed = 51; },
    (doc) => { doc.results[0].expected_report_sha256 = "0".repeat(64); },
    (doc) => { doc.results[0].replay_report_sha256 = null; },
    (doc) => { doc.results[0].moved = ["catalog 50→51"]; doc.results[0].drifted = true; doc.passed = false; },
    (doc) => { doc.results[0].reason = "evaluation-failed"; },
  ]) {
    const doc = structuredClone(receipt); corrupt(doc);
    await writeFile(marker, JSON.stringify(doc));
    await assert.rejects(checkEvidence(options));
  }
  const extended = structuredClone(receipt);
  extended.private_note = "PRIVATE_ROOT_METADATA";
  extended.evaluator.private_note = "PRIVATE_EVALUATOR_METADATA";
  extended.policy.private_note = "PRIVATE_POLICY_METADATA";
  extended.results[0].private_note = "PRIVATE_RESULT_METADATA";
  await writeFile(marker, JSON.stringify(extended));
  assert.deepEqual(await checkEvidence(options), receipt);
  await writeFile(marker, JSON.stringify(receipt));
  const replayFile = path.join(options.evidencePath, "reports/one.json");
  const original = await readFile(replayFile);
  await writeFile(replayFile, "changed replay");
  await assert.rejects(checkEvidence(options), /replay report digest mismatch/u);
  const modified = JSON.parse(original);
  modified.readiness.areas.find((area) => area.name === "catalog").score = 51;
  const raw = JSON.stringify(modified);
  await writeFile(replayFile, raw);
  await writeFile(marker, JSON.stringify({ ...receipt, results: [{ ...receipt.results[0], replay_report_sha256: sha256(raw) }] }));
  await assert.rejects(checkEvidence(options), /result does not match/u);
  await writeFile(marker, JSON.stringify(receipt));
  await rm(replayFile);
  await assert.rejects(checkEvidence(options), { code: "ENOENT" });
  await writeFile(replayFile, original);
  const extra = path.join(options.evidencePath, "reports/extra.json");
  await writeFile(extra, "PRIVATE_UNREFERENCED_REPORT");
  await assert.rejects(checkEvidence(options), /unexpected replay reports/u);
  await rm(extra);
  await rm(marker);
  await assert.rejects(checkEvidence(options), /incomplete evidence/u);
});

test("offline checking accepts reportless failures and rejects malformed failure reasons", async (t) => {
  let unavailable = false;
  const f = await fixture(t, [target("one")], (server) => unavailable ? "PRIVATE_INVALID_REPORT" : report(server));
  const collection = await collectCorpus(f.options);
  unavailable = true;
  const options = { ...f.options, corpusPath: f.options.output, reportsPath: collection.reports, evidencePath: path.join(f.dir, "evidence") };
  const receipt = await verifyCorpus(options);
  assert.equal(receipt.passed, false);
  assert.deepEqual(await checkEvidence(options), receipt);
  const marker = path.join(options.evidencePath, "verification.json");
  for (const reason of [["evaluation-failed"], "no-tool-calls", "unrecognized"]) {
    const doc = structuredClone(receipt); doc.results[0].reason = reason;
    await writeFile(marker, JSON.stringify(doc));
    await assert.rejects(checkEvidence(options));
  }
});

test("offline CLI rejects malformed private JSON without echoing it", async (t) => {
  const f = await fixture(t, [target("one")]);
  const collection = await collectCorpus(f.options);
  const options = { ...f.options, corpusPath: f.options.output, reportsPath: collection.reports, evidencePath: path.join(f.dir, "evidence") };
  await verifyCorpus(options);
  await writeFile(path.join(options.evidencePath, "verification.json"), "PRIVATE_INVALID_JSON");
  const cli = await runCommand([process.execPath, path.join(ROOT, "scripts/corpus/check-evidence.mjs"), "--corpus", options.corpusPath, "--targets", options.targetsPath, "--reports", options.reportsPath, "--evidence", options.evidencePath, "--json"]);
  assert.equal(cli.code, 2);
  assert.equal(cli.stdout, "");
  await assert.rejects(checkEvidence(options), (error) => !error.message.includes("PRIVATE_"));
});

test("offline checking binds original input bytes and projections and refuses linked evidence", async (t) => {
  const f = await fixture(t, [target("one")]);
  const collection = await collectCorpus(f.options);
  const options = { ...f.options, corpusPath: f.options.output, reportsPath: collection.reports, evidencePath: path.join(f.dir, "evidence") };
  await verifyCorpus(options);
  for (const file of [options.corpusPath, options.targetsPath, path.join(collection.reports, "one.json")]) {
    const original = await readFile(file);
    await writeFile(file, Buffer.concat([original, Buffer.from("\n")]));
    await assert.rejects(checkEvidence(options), /mismatch/u);
    await writeFile(file, "PRIVATE_INVALID_JSON");
    await assert.rejects(checkEvidence(options), (error) => !error.message.includes("PRIVATE_"));
    await writeFile(file, original);
  }
  assert.equal((await checkEvidence(options)).passed, true);
  const replayFile = path.join(options.evidencePath, "reports/one.json");
  const raw = await readFile(replayFile);
  await rm(replayFile);
  await mkdir(replayFile);
  await assert.rejects(checkEvidence(options), /regular files/u);
  await rm(replayFile, { recursive: true });
  if (process.platform !== "win32") {
    const external = path.join(f.dir, "external.json");
    await writeFile(external, raw);
    await symlink(external, replayFile);
    await assert.rejects(checkEvidence(options), /regular files/u);
    await rm(replayFile);
    const alias = path.join(f.dir, "bundle-link");
    await symlink(options.evidencePath, alias);
    await assert.rejects(checkEvidence({ ...options, evidencePath: alias }), /directories/u);
  }
  await writeFile(replayFile, raw);
  assert.equal((await checkEvidence(options)).passed, true);
});

test("target locks reject ranges, tags, duplicate labels, implicit prerequisites, and unknown runtimes", () => {
  for (const change of [{ version: "latest" }, { version: "^1.2.3" }, { version: "1.x" }, { runtime: "curl" }, { bin: "../secret" }, { prerequisites: undefined }]) {
    assert.throws(() => validateTargets({ schema: "mcpeval.corpus-targets/v1", standard: "mcpeval-standard/2", targets: [target("one", change)] }));
  }
  assert.throws(() => validateTargets({ schema: "mcpeval.corpus-targets/v1", standard: "mcpeval-standard/2", targets: [target("one"), target("one")] }));
  assert.deepEqual(commandFor(target("python", { runtime: "uvx", package: "fixture_python", version: "1.2.3rc1" })), ["uvx", "--from", "fixture_python==1.2.3rc1", "fixture-server"]);
});

test("only declared environment reaches a fresh home with isolation overrides", () => {
  const env = environmentFor(target("one", { prerequisites: { environment: ["FIXTURE_TOKEN"], checks: [] } }), "/fixture-home", { FIXTURE_TOKEN: "PRIVATE_TOKEN", OTHER_TOKEN: "PRIVATE_OTHER", HOME: "/private-original", DOCKER_HOST: "live-docker", KUBECONFIG: "live-cluster", PATH: "fixture-path" });
  assert.equal(env.FIXTURE_TOKEN, "PRIVATE_TOKEN"); assert.equal(env.OTHER_TOKEN, undefined);
  assert.equal(env.HOME, "/fixture-home"); assert.equal(env.DOCKER_HOST, "unix:///nonexistent/docker.sock");
  assert.equal(env.KUBECONFIG, path.join(ROOT, "scripts/corpus/empty-kubeconfig.yaml"));
});

test("drift retains the reliability tolerance and requires the original platform", () => {
  assert.deepEqual(driftOf({ areas: { reliability: 50, catalog: 50 } }, { areas: { reliability: 60, catalog: 50 } }), []);
  assert.deepEqual(driftOf({ areas: { reliability: 50, catalog: 50 } }, { areas: { reliability: 61, catalog: 51 } }), ["reliability 50→61", "catalog 50→51"]);
  assert.equal(platformMismatch({ platform: process.platform }, process.platform), null);
  assert.match(platformMismatch({ platform: "different" }, process.platform), /collected on/u);
});

test("process runner bounds stalled and oversized programs and reports missing executables", async () => {
  assert.equal((await runCommand([process.execPath, "-e", "setInterval(()=>{}, 1000)"], { timeoutMs: 100 })).reason, "timeout");
  assert.equal((await runCommand([process.execPath, "-e", "process.stdout.write('x'.repeat(4096)); setInterval(()=>{},1000)"], { maxBytes: 1024 })).reason, "output-limit");
  assert.equal((await runCommand(["mcpeval-fixture-does-not-exist-123"])).reason, "launch-failed");
});

test("collection refuses overwriting a candidate before launching and retains insufficient population", async (t) => {
  const f = await fixture(t, [target("one")]);
  const result = await collectCorpus({ ...f.options, minimum: 10 });
  assert.equal(result.sufficient, false);
  const before = f.launches.length;
  await assert.rejects(collectCorpus(f.options), /output exists/u);
  assert.equal(f.launches.length, before);
  const original = await readFile(f.options.output, "utf8");
  await collectCorpus({ ...f.options, force: true });
  assert.equal(await readFile(f.options.output, "utf8"), original);
});

test("a racing output writer is preserved without force", async (t) => {
  const f = await fixture(t, [target("one")]);
  const execute = async (command, options) => {
    if (command[1] === "--version") await writeFile(f.options.output, "ORIGINAL_RACING_WRITER");
    return f.options.execute(command, options);
  };
  await assert.rejects(collectCorpus({ ...f.options, execute }), { code: "EEXIST" });
  assert.equal(await readFile(f.options.output, "utf8"), "ORIGINAL_RACING_WRITER");
});

test("native evaluator and demo complete collection and artifact-checked replay through a fixture launcher", async (t) => {
  const binary = path.resolve(process.env.MCPEVAL_CORPUS_TEST_BINARY ?? path.join(ROOT, "target/release", process.platform === "win32" ? "mcpeval.exe" : "mcpeval"));
  const demo = path.join(path.dirname(binary), process.platform === "win32" ? "mcpeval-demo.exe" : "mcpeval-demo");
  const f = await fixture(t, [target("fixture-demo")]);
  // Replace only the package-launch boundary with the local demo fixture;
  // evaluator, JSON-RPC, subprocess lifecycle, reports, and replay are real.
  const execute = (command, options) => {
    if (command[0] === binary && command[1] === "score") return runCommand([...command.slice(0, command.indexOf("--")), "--", demo], options);
    return runCommand(command, options);
  };
  const options = { ...f.options, binary, execute, environment: process.env };
  const collection = await collectCorpus(options);
  assert.equal(collection.corpus.observations.length, 1);
  assert.ok(collection.corpus.observations[0].calls.successful_calls > 0);
  assert.deepEqual(driftedResults((await verifyCorpus({ ...options, corpusPath: options.output, reportsPath: collection.reports })).results), []);
});

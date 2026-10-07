import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { commandFor, readinessScore, driftedResults } from "./verify.mjs";
import { validateCorpus } from "./contract.mjs";

const corpusFixture = JSON.parse(await readFile(new URL("../../tests/fixtures/corpus-v3.json", import.meta.url), "utf8"));
const metadataCases = JSON.parse(await readFile(new URL("../../tests/fixtures/corpus-v3-metadata-cases.json", import.meta.url), "utf8"));

for (const scenario of metadataCases) {
  test(`corpus metadata contract: ${scenario.name}`, () => {
    const document = { ...structuredClone(corpusFixture), ...scenario.set };
    for (const key of scenario.remove) delete document[key];
    if (scenario.valid) assert.equal(validateCorpus(document), document);
    else assert.throws(() => validateCorpus(document));
  });
}

test("launches only an explicitly pinned package target", () => {
  const target = { server: "memory", runtime: "npm", package: "@modelcontextprotocol/server-memory", version: "1.2.3", bin: "fixture-memory", args: [], prerequisites: { environment: [], checks: [] } };
  assert.deepEqual(commandFor(target), ["npx", "--yes", "--package", "@modelcontextprotocol/server-memory@1.2.3", "--", "fixture-memory"]);
  assert.throws(() => commandFor({ ...target, version: "latest" }));
});

test("native targets require an immutable deployment rather than a package runner", () => {
  const target = { server: "bobby", runtime: "native", package: "bobby-browser", version: "0.19.1", bin: "mcp-gateway", args: [],
    deployment: { root: "/private/reference", executable: "mcp-gateway", sha256: "a".repeat(64) } };
  assert.deepEqual(commandFor(target), ["/private/reference/mcp-gateway"]);
  const { deployment, ...unlocked } = target;
  assert.throws(() => commandFor(unlocked), /deployment/);
  assert.throws(() => commandFor({ ...target, version: "latest" }));
});

test("out of range and fractional scores are not observations", () => {
  for (const score of [-1, 101, 20.5, "40", null]) {
    assert.equal(readinessScore({ readiness: { score } }), null);
  }
  assert.equal(readinessScore({ readiness: { score: 0 } }), 0);
});

test("unrun and unknown targets cannot pass drift verification", () => {
  const results = [ { status: "unknown-server" }, { status: "untested" }, { status: "errored" } ];
  assert.deepEqual(driftedResults(results), results);
});

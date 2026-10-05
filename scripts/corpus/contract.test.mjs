import assert from "node:assert/strict";
import test from "node:test";
import { commandFor, readinessScore, driftedResults } from "./verify.mjs";

test("launches only an explicitly pinned package target", () => {
  const target = { server: "memory", runtime: "npm", package: "@modelcontextprotocol/server-memory", version: "1.2.3", bin: "fixture-memory", args: [], prerequisites: { environment: [], checks: [] } };
  assert.deepEqual(commandFor(target), ["npx", "--yes", "--package", "@modelcontextprotocol/server-memory@1.2.3", "--", "fixture-memory"]);
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

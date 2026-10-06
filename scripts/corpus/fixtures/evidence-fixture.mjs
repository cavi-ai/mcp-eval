import { mkdtemp, writeFile, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { AREAS, COUNTS } from "../contract.mjs";

export const target = (server, extra = {}) => ({ server, runtime: "npm", package: "fixture-package", version: "1.2.3", bin: "fixture-server", args: [], prerequisites: { environment: [], checks: [] }, ...extra });
export const report = (server, changes = {}) => ({ schema: "mcpeval.probe-report/v2", server, generator: { name: "mcpeval", version: "0.4.0" }, gate: null, cases: [], passed: true, readiness: {
  standard: "mcpeval-standard/2", score: 50, surface: { tools: 1, read_only: 1, writers: 0, exercised: 0 },
  areas: AREAS.map((name) => ({ name, score: 50, measurements: name === "reliability" ? Object.fromEntries(COUNTS.map((key) => [key, key === "tool_errors" ? 3 : 0])) : name === "context" ? { catalog_tokens: 100 } : {} })), ...changes,
} });

export async function fixture(t, targets, evaluate = (server) => report(server)) {
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

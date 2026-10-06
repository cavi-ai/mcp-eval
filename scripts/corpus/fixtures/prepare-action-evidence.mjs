// Synthetic native-demo fixtures for qualifying the evidence action on a runner.
import { chmod, copyFile, mkdir, rename, rm, writeFile } from "node:fs/promises";
import path from "node:path";
import { parseArgs } from "node:util";
import { ROOT } from "../contract.mjs";
import { deploymentDigest } from "../deployment.mjs";
import { collectCorpus } from "../collect.mjs";
import { verifyCorpus } from "../verify.mjs";

const { values } = parseArgs({ options: { out: { type: "string" }, binary: { type: "string" } } });
if (!values.out) throw new Error("--out is required");
const root = path.resolve(values.out);
const binary = path.resolve(values.binary ?? path.join(ROOT, "target/release", process.platform === "win32" ? "mcpeval.exe" : "mcpeval"));
await mkdir(root, { mode: 0o700 }); // Refuse to replace an existing fixture set.
try {
  const deployment = path.join(root, "deployment"); await mkdir(deployment);
  await copyFile(path.join(path.dirname(binary), process.platform === "win32" ? "mcpeval-demo.exe" : "mcpeval-demo"), path.join(deployment, "runtime"));
  await chmod(path.join(deployment, "runtime"), 0o755);
  await writeFile(path.join(deployment, "dependency"), "synthetic locked dependency");
  const targetsPath = path.join(root, "targets.json");
  await writeFile(targetsPath, JSON.stringify({ schema: "mcpeval.corpus-targets/v1", standard: "mcpeval-standard/2", targets: [{
    server: "fixture", runtime: "npm", package: "fixture-package", version: "1.2.3", bin: "fixture", args: [],
    prerequisites: { environment: [], checks: [] }, deployment: { root: deployment, sha256: await deploymentDigest(deployment), executable: "runtime" },
  }] }));
  const options = { targetsPath, binary, output: path.join(root, "corpus.json"), minimum: 1 };
  const collection = await collectCorpus(options);
  if (!collection.sufficient) throw new Error("native fixture collection failed");
  const reportsPath = path.join(root, "original-reports"); await rename(collection.reports, reportsPath);
  const replay = { ...options, corpusPath: options.output, reportsPath };
  for (const name of ["passing", "invalid"]) {
    if (!(await verifyCorpus({ ...replay, evidencePath: path.join(root, name) })).passed) throw new Error("native fixture replay failed");
  }
  await writeFile(path.join(root, "invalid/verification.json"), "invalid synthetic evidence");
  await writeFile(path.join(deployment, "dependency"), "changed synthetic dependency");
  if ((await verifyCorpus({ ...replay, evidencePath: path.join(root, "failed") })).passed) throw new Error("changed fixture deployment passed replay");
  console.log("Prepared synthetic corpus evidence action fixtures.");
} catch (error) {
  await rm(root, { recursive: true, force: true });
  throw error;
}

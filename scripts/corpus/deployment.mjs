import { createHash } from "node:crypto";
import { createReadStream } from "node:fs";
import { lstat, readdir, readlink, realpath, stat } from "node:fs/promises";
import path from "node:path";
import { parseArgs } from "node:util";

const relativeFile = (value) => typeof value === "string" && value.length > 0 && value.length <= 4096 && !/[\\\0]/u.test(value) && !path.isAbsolute(value) && value.split("/").every((part) => part && part !== "." && part !== "..");
export function validateDeployment(deployment) {
  if (!deployment || typeof deployment.root !== "string" || !path.isAbsolute(deployment.root) || deployment.root.includes("\0") || typeof deployment.sha256 !== "string" || !/^[a-f0-9]{64}$/u.test(deployment.sha256) || !relativeFile(deployment.executable) || (deployment.entrypoint !== undefined && !relativeFile(deployment.entrypoint))) throw new Error("invalid deployment lock");
  return deployment;
}

/** Versioned, location-independent fingerprint of a prepared execution tree. */
export async function deploymentDigest(root) {
  if (!(await lstat(root)).isDirectory()) throw new Error("deployment root must be a directory, not a link");
  const base = await realpath(root);
  const hash = createHash("sha256").update("mcpeval.deployment-tree/v1\n");
  const record = (...values) => hash.update(`${JSON.stringify(values)}\n`);
  async function visit(relative) {
    const file = path.join(base, relative);
    const info = await lstat(file);
    if (info.isSymbolicLink()) {
      const link = await readlink(file);
      const destination = path.relative(base, await realpath(file));
      if (path.isAbsolute(link) || destination === ".." || destination.startsWith(`..${path.sep}`) || path.isAbsolute(destination)) throw new Error("link points outside deployment or is not relocatable");
      record("link", relative, link);
    } else if (info.isDirectory()) {
      record("directory", relative, info.mode & 0o777);
      for (const name of (await readdir(file)).sort()) await visit(relative ? `${relative}/${name}` : name);
    } else if (info.isFile()) {
      const content = createHash("sha256");
      for await (const bytes of createReadStream(file)) content.update(bytes);
      record("file", relative, info.mode & 0o777, content.digest("hex"));
    } else throw new Error("deployment contains a non-file entry");
  }
  await visit("");
  return hash.digest("hex");
}

export async function deploymentMatches(deployment) {
  if (deployment === undefined) return true;
  try {
    validateDeployment(deployment);
    if (await deploymentDigest(deployment.root) !== deployment.sha256) return false;
    for (const relative of [deployment.executable, deployment.entrypoint].filter((value) => value !== undefined)) {
      if (!(await stat(path.join(deployment.root, relative))).isFile()) return false;
    }
    return true;
  } catch { return false; }
}

if (process.argv[1] && path.resolve(process.argv[1]) === import.meta.filename) {
  try {
    const { values } = parseArgs({ options: { root: { type: "string" } } });
    if (!values.root || !path.isAbsolute(values.root)) throw new Error("--root requires an absolute deployment directory");
    console.log(await deploymentDigest(values.root));
  } catch (error) { console.error(error.message); process.exitCode = 1; }
}

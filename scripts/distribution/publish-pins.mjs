#!/usr/bin/env node
// Publishes a release's distribution files: the committed manifest and formula
// in this repository, and the formula in the Homebrew tap. Each lands as one
// GitHub-signed commit on a branch cut from the default branch and merges
// through a pull request; the calling workflow has already verified the files
// against the published archives and installed the formula through Homebrew.
import { execFile } from "node:child_process";
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { updateFormulaIndex } from "./stage-homebrew.mjs";

const MODULE_PATH = fileURLToPath(import.meta.url);
const SOURCE = "cavi-ai/mcp-eval";
const TAP = "cavi-ai/homebrew-tap";
const MANIFEST = "distribution/release.json";
const FORMULA = "Formula/mcpeval.rb";
const README = "README.md";
const RELEASE_URL = /\/releases\/download\/v(\d+\.\d+\.\d+)\//gu;
const COMMIT_MUTATION = `mutation($input: CreateCommitOnBranchInput!) {
  createCommitOnBranch(input: $input) { commit { oid } }
}`;

/** The one release version a manifest or formula describes. */
export function describedVersion(file, content) {
  if (file === MANIFEST) return JSON.parse(content).version;
  const versions = new Set([...content.matchAll(RELEASE_URL)].map((match) => match[1]));
  if (versions.size !== 1) throw new Error(`${file} must name one release version`);
  return [...versions][0];
}

function compareVersions(left, right) {
  const [a, b] = [left, right].map((version) => version.split(".").map(Number));
  return a[0] - b[0] || a[1] - b[1] || a[2] - b[2];
}

/**
 * `update` when the candidate files carry a newer release, `current` when
 * nothing differs, `newer` when the repository already pins a later release.
 * A different file for an already published version is refused.
 */
export function planUpdate(current, candidate) {
  const changed = Object.keys(candidate).filter((file) => current[file] !== candidate[file]);
  if (changed.length === 0) return { action: "current", changed };
  for (const file of changed) {
    if (file === README || current[file] == null) continue;
    const order = compareVersions(describedVersion(file, current[file]), describedVersion(file, candidate[file]));
    if (order > 0) return { action: "newer", changed };
    if (order === 0) throw new Error(`refusing to change ${file} of an already published version`);
  }
  return { action: "update", changed };
}

function body(change) {
  return `## What changed

- ${change}

## Why

- Default installers must resolve the current release.

## Verification

- All archive SHA-256 digests match the published release (\`verify.mjs --online --strict\`).
- The formula passed a Homebrew install and test on macOS.

## Contract impact

- No report or manifest schema change.
`;
}

/** The files each repository pins for `version`, read from `root`. */
export async function releaseTargets({ root, version }) {
  const manifest = await readFile(path.join(root, MANIFEST), "utf8");
  const formula = await readFile(path.join(root, FORMULA), "utf8");
  for (const [file, content] of [[MANIFEST, manifest], [FORMULA, formula]]) {
    if (describedVersion(file, content) !== version) throw new Error(`${file} does not describe ${version}`);
  }
  return [
    {
      repository: SOURCE,
      branch: `chore/distribution-${version}`,
      title: `chore: update ${version} distribution`,
      body: body(`Refresh the distribution manifest and Homebrew formula with the published v${version} archives and checksums.`),
      files: { [MANIFEST]: manifest, [FORMULA]: formula },
    },
    {
      repository: TAP,
      branch: `chore/mcpeval-${version}`,
      title: `chore: update mcpeval to ${version}`,
      body: body(`Update the mcpeval formula to ${version} with verified release archive hashes.`),
      files: { [FORMULA]: formula },
      readme: true,
    },
  ];
}

function gh(args, input) {
  return new Promise((resolve, reject) => {
    const child = execFile("gh", args, { maxBuffer: 64 * 1024 * 1024, timeout: 120_000 }, (error, stdout, stderr) => {
      if (error) reject(new Error((stderr || stdout || error.message).trim()));
      else resolve(stdout);
    });
    child.stdin.end(input === undefined ? "" : JSON.stringify(input));
  });
}

async function api(endpoint, payload) {
  const args = ["api", endpoint, ...(payload === undefined ? [] : ["--method", "POST", "--input", "-"])];
  return JSON.parse(await gh(args, payload));
}

async function readText(repository, file, ref) {
  try {
    const content = await api(`repos/${repository}/contents/${file}?ref=${ref}`);
    return Buffer.from(content.content, "base64").toString("utf8");
  } catch (error) {
    if (/HTTP 404/u.test(error.message)) return null;
    throw error;
  }
}

async function verifySigned(repository, commit, parent) {
  const record = await api(`repos/${repository}/commits/${commit}`);
  if (record.commit.verification?.verified !== true) throw new Error(`${repository}@${commit} has no verified signature`);
  if (record.parents.map((entry) => entry.sha).join() !== parent) throw new Error(`${repository}@${commit} does not start at ${parent}`);
}

async function holds(repository, ref, files) {
  const contents = await Promise.all(Object.keys(files).map((file) => readText(repository, file, ref)));
  return Object.values(files).every((content, index) => contents[index] === content);
}

export async function publishTarget(target, { dryRun = false } = {}) {
  const { repository } = target;
  const owner = repository.split("/")[0];
  const defaultBranch = (await api(`repos/${repository}`)).default_branch;
  const baseRef = `repos/${repository}/git/ref/heads/${defaultBranch}`;
  const base = (await api(baseRef)).object.sha;
  const candidate = { ...target.files };
  if (target.readme) {
    const readme = await readText(repository, README, base);
    const next = updateFormulaIndex(readme);
    if (next !== readme) candidate[README] = next;
  }
  const current = {};
  for (const file of Object.keys(candidate)) current[file] = await readText(repository, file, base);
  const plan = planUpdate(current, candidate);
  if (plan.action !== "update" || dryRun) {
    console.log(`${repository}: ${plan.action}${plan.changed.length ? ` ${plan.changed.join(", ")}` : ""}${dryRun ? " (dry run)" : ""}`);
    return null;
  }
  const changes = Object.fromEntries(plan.changed.map((file) => [file, candidate[file]]));

  // A moved default branch gets a new branch, so a retry never reuses a
  // commit built on an earlier base.
  const branch = `${target.branch}-${base.slice(0, 12)}`;
  const refs = await api(`repos/${repository}/git/matching-refs/heads/${branch}`);
  let head = refs.find((ref) => ref.ref === `refs/heads/${branch}`)?.object.sha;
  if (head === undefined) {
    await api(`repos/${repository}/git/refs`, { ref: `refs/heads/${branch}`, sha: base });
    head = base;
  }
  if (head !== base) {
    const files = (await api(`repos/${repository}/compare/${base}...${head}`)).files.map((file) => file.filename);
    if (files.sort().join() !== plan.changed.sort().join() || !(await holds(repository, head, changes))) {
      throw new Error(`${repository}: ${branch} already holds a different change`);
    }
  } else {
    const result = await api("graphql", {
      query: COMMIT_MUTATION,
      variables: {
        input: {
          branch: { repositoryNameWithOwner: repository, branchName: branch },
          expectedHeadOid: base,
          message: { headline: target.title },
          fileChanges: {
            additions: plan.changed.map((file) => ({ path: file, contents: Buffer.from(candidate[file]).toString("base64") })),
          },
        },
      },
    });
    if (result.errors) throw new Error(JSON.stringify(result.errors));
    head = result.data.createCommitOnBranch.commit.oid;
  }
  await verifySigned(repository, head, base);

  const open = await api(`repos/${repository}/pulls?state=open&head=${owner}:${branch}&base=${defaultBranch}`);
  let number = open[0]?.number;
  if (number === undefined) {
    const url = (await gh(["pr", "create", "--repo", repository, "--head", branch, "--base", defaultBranch, "--title", target.title, "--body", target.body])).trim();
    number = Number(url.slice(url.lastIndexOf("/") + 1));
  } else if (open[0].head.sha !== head) {
    throw new Error(`${repository}#${number} head changed`);
  }
  console.log(`https://github.com/${repository}/pull/${number}`);

  if ((await api(baseRef)).object.sha !== base) throw new Error(`${repository}: ${defaultBranch} moved during publication; rerun`);
  await gh(["pr", "merge", String(number), "--repo", repository, "--squash", "--match-head-commit", head, "--subject", target.title, "--body", ""]);
  const pull = await api(`repos/${repository}/pulls/${number}`);
  if (!pull.merged || !(await holds(repository, pull.merge_commit_sha, changes))) {
    throw new Error(`${repository}#${number} did not merge with the expected files`);
  }
  await verifySigned(repository, pull.merge_commit_sha, base);
  console.log(`${repository}: merged #${number}`);
  return number;
}

function option(args, flag) {
  const index = args.indexOf(flag);
  if (index === -1 || !args[index + 1]) throw new Error(`${flag} is required`);
  return args[index + 1];
}

async function main() {
  const args = process.argv.slice(2);
  const tag = option(args, "--tag");
  const match = /^v(\d+\.\d+\.\d+)$/u.exec(tag);
  if (!match) throw new Error(`${tag} is not a release tag`);
  const root = args.includes("--root") ? path.resolve(option(args, "--root")) : path.resolve(path.dirname(MODULE_PATH), "../..");
  const dryRun = args.includes("--dry-run");
  for (const target of await releaseTargets({ root, version: match[1] })) await publishTarget(target, { dryRun });
}

if (process.argv[1] && path.resolve(process.argv[1]) === MODULE_PATH) {
  main().catch((error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
}

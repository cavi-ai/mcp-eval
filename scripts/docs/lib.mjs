import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { lstat, readFile, readdir } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

export const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
export const PRODUCT_ID = "mcp-eval";
export const SOURCE_REL = `docs/${PRODUCT_ID}/source`;
export const PRODUCT_VERSION_TOKEN = "{{PRODUCT_VERSION}}";
export const RELEASE_TAG_TOKEN = "{{RELEASE_TAG}}";
export const RELEASE_COMMIT_TOKEN = "{{RELEASE_COMMIT}}";

function tomlSection(source, header) {
  const start = source.indexOf(`${header}\n`);
  if (start < 0) throw new Error(`Cargo.toml is missing ${header}`);
  const bodyStart = start + header.length + 1;
  const next = source.slice(bodyStart).search(/^\[/mu);
  return next < 0 ? source.slice(bodyStart) : source.slice(bodyStart, bodyStart + next);
}

function tomlString(section, field) {
  const match = section.match(new RegExp(`^${field}\\s*=\\s*"([^"]+)"\\s*$`, "mu"));
  if (!match) throw new Error(`Cargo.toml ${field} is required`);
  return match[1];
}

function cargoMetadata() {
  const source = readFileSync(path.join(REPO_ROOT, "Cargo.toml"), "utf8");
  const packageSection = tomlSection(source, "[package]");
  const binarySection = tomlSection(source, "[[bin]]");
  const repository = tomlString(packageSection, "repository")
    .replace(/^https:\/\/github\.com\//u, "")
    .replace(/\.git$/u, "");
  return {
    binary: tomlString(binarySection, "name"),
    package: tomlString(packageSection, "name"),
    repository,
    version: tomlString(packageSection, "version"),
  };
}

const CARGO = cargoMetadata();
export const CLI_BINARY = CARGO.binary;
export const PACKAGE_ID = CARGO.package;
export const RELEASE_REPOSITORY = CARGO.repository;
export const DOCUMENTED_VERSION = CARGO.version;
export const OUTPUT_REL = `docs/${PRODUCT_ID}/v${DOCUMENTED_VERSION}`;

const STABLE_VERSION = /^(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)$/u;
const COMMIT_SHA = /^[a-f0-9]{40}$/u;
export const UNRESOLVED_TOKEN = /\{\{[A-Z][A-Z0-9_]*\}\}/u;

export function comparePortablePaths(left, right) {
  return left < right ? -1 : left > right ? 1 : 0;
}

function git(command, args) {
  return execFileSync("git", [command, ...args], {
    cwd: REPO_ROOT,
    encoding: "utf8",
  }).trim();
}

export function defaultReleaseIdentity() {
  const commit = git("rev-parse", ["HEAD"]);
  const sourceDateEpoch = Number(git("show", ["-s", "--format=%ct", commit]));
  return resolveReleaseIdentity({
    version: DOCUMENTED_VERSION,
    tag: `v${DOCUMENTED_VERSION}`,
    commit,
    sourceDateEpoch,
  });
}

export function resolveReleaseIdentity(input = {}) {
  const defaults = Object.keys(input).length ? null : defaultReleaseIdentity();
  const version = input.version ?? defaults?.version ?? DOCUMENTED_VERSION;
  const tag = input.tag ?? defaults?.tag ?? `v${version}`;
  const commit = input.commit ?? defaults?.commit;
  const sourceDateEpoch = input.sourceDateEpoch ?? defaults?.sourceDateEpoch;
  if (version !== DOCUMENTED_VERSION || !STABLE_VERSION.test(version)) {
    throw new Error(`release version must be ${DOCUMENTED_VERSION}`);
  }
  if (tag !== `v${version}`) throw new Error("release tag must match version");
  if (!COMMIT_SHA.test(commit ?? "")) {
    throw new Error("release commit must be a full lowercase SHA");
  }
  if (!Number.isSafeInteger(sourceDateEpoch) || sourceDateEpoch < 0) {
    throw new Error("source date epoch must be a non-negative integer");
  }
  return Object.freeze({
    version,
    tag,
    commit,
    sourceDateEpoch,
    generatedAt: new Date(sourceDateEpoch * 1000).toISOString(),
  });
}

export function stampReleaseTokens(text, release) {
  return text
    .replaceAll(PRODUCT_VERSION_TOKEN, release.version)
    .replaceAll(RELEASE_TAG_TOKEN, release.tag)
    .replaceAll(RELEASE_COMMIT_TOKEN, release.commit);
}

export function parseOptions(argv) {
  const values = {};
  for (let index = 0; index < argv.length; index += 1) {
    const option = argv[index];
    const value = argv[index + 1];
    if (option.startsWith("--") && value && !value.startsWith("--")) {
      values[option.slice(2)] = value;
      index += 1;
    }
  }
  return values;
}

export function releaseInput(values) {
  return {
    ...(values.version !== undefined ? { version: values.version } : {}),
    ...(values.tag !== undefined ? { tag: values.tag } : {}),
    ...(values.commit !== undefined ? { commit: values.commit } : {}),
    ...(values["source-date-epoch"] !== undefined
      ? { sourceDateEpoch: Number(values["source-date-epoch"]) }
      : {}),
  };
}

export async function listFilesRecursive(root) {
  const files = [];
  async function walk(directory) {
    const entries = await readdir(directory, { withFileTypes: true });
    entries.sort((left, right) => comparePortablePaths(left.name, right.name));
    for (const entry of entries) {
      const absolute = path.join(directory, entry.name);
      const info = await lstat(absolute);
      const relative = path.relative(root, absolute).split(path.sep).join("/");
      if (info.isSymbolicLink()) throw new Error(`documentation rejects symlink: ${relative}`);
      if (info.isDirectory()) await walk(absolute);
      else if (info.isFile()) files.push(relative);
      else throw new Error(`documentation rejects non-file: ${relative}`);
    }
  }
  await walk(root);
  return files.sort(comparePortablePaths);
}

export async function computeContentSha256(root, relativePaths) {
  const hash = createHash("sha256");
  for (const relative of relativePaths
    .filter((item) => item !== "manifest.json")
    .sort(comparePortablePaths)) {
    hash.update(relative);
    hash.update("\0");
    hash.update(await readFile(path.join(root, relative)));
    hash.update("\0");
  }
  return hash.digest("hex");
}

export function navigationPaths(navigation) {
  if (!navigation || !Array.isArray(navigation.sections)) {
    throw new Error("navigation.json missing sections array");
  }
  const pages = navigation.sections.flatMap((section) => (
    Array.isArray(section.pages) ? section.pages.map((page) => page.path) : []
  ));
  if (pages.some((page) => typeof page !== "string" || !page || page.startsWith("/") || page.split("/").includes(".."))) {
    throw new Error("navigation.json contains an unsafe page path");
  }
  if (new Set(pages).size !== pages.length) throw new Error("navigation.json contains duplicate page paths");
  return pages;
}

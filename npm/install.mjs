import { createHash } from "node:crypto";
import { chmod, mkdtemp, mkdir, readFile, rename, rm, writeFile } from "node:fs/promises";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const MODULE_PATH = fileURLToPath(import.meta.url);
const DEFAULT_PACKAGE_ROOT = path.resolve(path.dirname(MODULE_PATH), "..");

function assetFor(manifest, platform, arch) {
  const key = `${platform}-${arch}`;
  const asset = manifest.assets?.[key];
  if (!asset) throw new Error(`mcp-eval: unsupported platform ${key}`);
  return asset;
}

async function download(url) {
  const response = await fetch(url, { redirect: "follow" });
  if (!response.ok) throw new Error(`mcp-eval: download failed (${response.status}) for ${url}`);
  return Buffer.from(await response.arrayBuffer());
}

function verifyCompanion(bytes, asset) {
  const text = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  const match = /^([a-f0-9]{64})  ([^\r\n]+)\r?\n?$/u.exec(text);
  if (!match || match[2] !== asset.archive) {
    throw new Error(`mcp-eval: invalid checksum companion for ${asset.archive}`);
  }
  if (match[1] !== asset.sha256) {
    throw new Error(`mcp-eval: checksum companion does not match the pinned SHA-256 for ${asset.archive}`);
  }
}

function extractBinary(archivePath, destination, binaryName) {
  const args = archivePath.endsWith(".zip")
    ? ["-xf", archivePath, "-C", destination, binaryName]
    : ["-xzf", archivePath, "-C", destination, binaryName];
  const extracted = spawnSync("tar", args, { encoding: "utf8" });
  if (extracted.error) throw new Error(`mcp-eval: could not run tar: ${extracted.error.message}`);
  if (extracted.status !== 0) {
    throw new Error(`mcp-eval: archive extraction failed: ${extracted.stderr.trim()}`);
  }
}

export async function installRelease({
  manifest,
  packageRoot = DEFAULT_PACKAGE_ROOT,
  platform = process.platform,
  arch = process.arch,
  downloadBaseUrl,
}) {
  const asset = assetFor(manifest, platform, arch);
  const baseUrl = downloadBaseUrl
    ?? `https://github.com/${manifest.repository}/releases/download/${manifest.tag}`;
  const assetUrl = `${baseUrl}/${asset.archive}`;
  const checksumUrl = `${assetUrl}.sha256`;
  const npmRoot = path.join(packageRoot, "npm");
  const vendorRoot = path.join(npmRoot, "vendor");
  await mkdir(npmRoot, { recursive: true });
  const staging = await mkdtemp(path.join(npmRoot, ".mcpeval-install-"));
  let installed = false;

  try {
    verifyCompanion(await download(checksumUrl), asset);
    const archive = await download(assetUrl);
    const actual = createHash("sha256").update(archive).digest("hex");
    if (actual !== asset.sha256) {
      throw new Error(`mcp-eval: archive SHA-256 mismatch for ${asset.archive}`);
    }

    const archivePath = path.join(staging, asset.archive);
    await writeFile(archivePath, archive);
    const binaryName = platform === "win32" ? "mcpeval.exe" : "mcpeval";
    extractBinary(archivePath, staging, binaryName);
    await rm(archivePath, { force: true });
    if (platform !== "win32") await chmod(path.join(staging, binaryName), 0o755);
    await writeFile(path.join(staging, ".version"), `${manifest.tag} ${asset.target}\n`);

    await rm(vendorRoot, { recursive: true, force: true });
    await rename(staging, vendorRoot);
    installed = true;
    return { binary: path.join(vendorRoot, binaryName), target: asset.target };
  } finally {
    if (!installed) await rm(staging, { recursive: true, force: true });
  }
}

async function main() {
  const manifest = JSON.parse(await readFile(path.join(DEFAULT_PACKAGE_ROOT, "distribution/release.json"), "utf8"));
  const result = await installRelease({ manifest });
  console.log(`mcp-eval: installed ${manifest.version} (${result.target})`);
}

if (process.argv[1] && path.resolve(process.argv[1]) === MODULE_PATH) {
  main().catch((error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
}

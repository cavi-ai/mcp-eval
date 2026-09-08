#!/usr/bin/env node
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { renderFormula } from "./verify.mjs";

const MODULE_PATH = fileURLToPath(import.meta.url);
const DEFAULT_MANIFEST = path.resolve(path.dirname(MODULE_PATH), "../../distribution/release.json");

/** Render `Formula/mcpeval.rb` from a distribution manifest. */
export async function renderFormulaFile(manifestPath = DEFAULT_MANIFEST) {
  return renderFormula(JSON.parse(await readFile(manifestPath, "utf8")));
}

async function main() {
  const args = process.argv.slice(2);
  const index = args.indexOf("--manifest");
  const manifestPath = index === -1 ? DEFAULT_MANIFEST : path.resolve(args[index + 1] ?? "");
  process.stdout.write(await renderFormulaFile(manifestPath));
}

if (process.argv[1] && path.resolve(process.argv[1]) === MODULE_PATH) {
  main().catch((error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
}

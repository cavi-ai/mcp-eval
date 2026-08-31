#!/usr/bin/env node
import { mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const MODULE_PATH = fileURLToPath(import.meta.url);
const FORMULA_ROW = "| `mcpeval` | `mcpeval`, `mcpeval-demo` | [cavi-ai/mcp-eval](https://github.com/cavi-ai/mcp-eval) |";

async function readOptional(file) {
  try {
    return await readFile(file, "utf8");
  } catch (error) {
    if (error.code === "ENOENT") return null;
    throw error;
  }
}

function updateFormulaIndex(readme) {
  const lines = readme.split("\n");
  const existing = lines.findIndex((line) => line.startsWith("| `mcpeval` |"));
  if (existing !== -1) {
    lines[existing] = FORMULA_ROW;
    return lines.join("\n");
  }

  const heading = lines.indexOf("## Formulae");
  if (heading === -1) throw new Error("tap README has no Formulae section");
  const separator = lines.findIndex((line, index) => index > heading && /^\|[-|]+\|$/u.test(line.replaceAll(" ", "")));
  if (separator === -1) throw new Error("tap README has no Formulae table");
  let insertion = separator + 1;
  while (insertion < lines.length && lines[insertion].startsWith("|")) insertion += 1;
  lines.splice(insertion, 0, FORMULA_ROW);
  return lines.join("\n");
}

export async function stageHomebrewTap({ sourceRoot, tapRoot }) {
  const sourceFormula = await readFile(path.join(sourceRoot, "Formula/mcpeval.rb"), "utf8");
  const tapFormula = path.join(tapRoot, "Formula/mcpeval.rb");
  const previousFormula = await readOptional(tapFormula);
  const formulaChanged = previousFormula !== sourceFormula;
  if (formulaChanged) {
    await mkdir(path.dirname(tapFormula), { recursive: true });
    await writeFile(tapFormula, sourceFormula);
  }

  const readmePath = path.join(tapRoot, "README.md");
  const previousReadme = await readFile(readmePath, "utf8");
  const nextReadme = updateFormulaIndex(previousReadme);
  const readmeChanged = previousReadme !== nextReadme;
  if (readmeChanged) await writeFile(readmePath, nextReadme);
  return { formulaChanged, readmeChanged };
}

function value(args, flag) {
  const index = args.indexOf(flag);
  if (index === -1 || !args[index + 1]) throw new Error(`${flag} is required`);
  return path.resolve(args[index + 1]);
}

async function main() {
  const result = await stageHomebrewTap({
    sourceRoot: value(process.argv.slice(2), "--source"),
    tapRoot: value(process.argv.slice(2), "--tap"),
  });
  console.log(JSON.stringify(result));
}

if (process.argv[1] && path.resolve(process.argv[1]) === MODULE_PATH) {
  main().catch((error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
}

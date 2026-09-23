import { spawn } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.dirname(fileURLToPath(import.meta.url));

export function launch(name) {
  const binary = path.join(root, "vendor", process.platform === "win32" ? `${name}.exe` : name);
  const child = spawn(binary, process.argv.slice(2), { stdio: "inherit" });

  child.on("error", (error) => {
    console.error(`mcp-eval: failed to launch installed binary: ${error.message}`);
    process.exitCode = 1;
  });

  child.on("exit", (code, signal) => {
    if (signal) {
      process.kill(process.pid, signal);
      return;
    }
    process.exitCode = code ?? 1;
  });
}

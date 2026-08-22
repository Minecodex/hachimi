import { spawnSync } from "node:child_process";

// Runs a PowerShell script on Windows and exits 0 with a clear message on
// other platforms, so package.json entries stay platform-neutral.

const script = process.argv[2];
const arguments_ = process.argv.slice(3);

if (!script) {
  console.error("Usage: node scripts/windows-only.mjs <script.ps1> [args]");
  process.exit(2);
}

if (process.platform !== "win32") {
  console.log(`${script} is Windows-only; skipping on ${process.platform}.`);
  process.exit(0);
}

const result = spawnSync(
  "powershell.exe",
  ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", script, ...arguments_],
  { stdio: "inherit", windowsHide: true },
);
if (result.error) {
  throw new Error(`Unable to start powershell.exe: ${result.error.message}`);
}
process.exit(result.status ?? 1);

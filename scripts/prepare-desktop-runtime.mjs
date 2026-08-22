import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

// Prepares platform-specific desktop runtime assets before `tauri dev` / `tauri build`.
// Windows builds the CEF embedded browser runtime; macOS is intentionally degraded
// until the P4 CEF port (docs/mac-plan/phase-4-cef-embedded-browser.md).

const scriptsDirectory = dirname(fileURLToPath(import.meta.url));
const workspaceRoot = resolve(scriptsDirectory, "..");
const mode = process.argv[2];

if (mode !== "dev" && mode !== "release") {
  console.error("Usage: node scripts/prepare-desktop-runtime.mjs <dev|release>");
  process.exit(2);
}

if (process.platform === "win32") {
  const arguments_ = [
    "-NoProfile",
    "-ExecutionPolicy",
    "Bypass",
    "-File",
    join(scriptsDirectory, "build-cef-host.ps1"),
  ];
  if (mode === "release") arguments_.push("-Release");
  const result = spawnSync("powershell.exe", arguments_, {
    cwd: workspaceRoot,
    stdio: "inherit",
    windowsHide: true,
  });
  if (result.error) {
    throw new Error(`Unable to start powershell.exe: ${result.error.message}`);
  }
  process.exit(result.status ?? 1);
}

// tauri.conf.json maps target/cef-bundle/ as a bundle resource; the directory
// must exist even though the CEF runtime is not staged on this platform.
const bundleDirectory = join(workspaceRoot, "target", "cef-bundle");
mkdirSync(bundleDirectory, { recursive: true });
const keep = join(bundleDirectory, ".gitkeep");
if (!existsSync(keep)) writeFileSync(keep, "");
console.log(
  "CEF embedded browser runtime is Windows-only for now; skipping " +
    "(see docs/mac-plan/phase-4-cef-embedded-browser.md).",
);

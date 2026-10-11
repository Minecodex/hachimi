import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../../..");
const buildTarget = join(root, "target/desktop-e2e-build");
const application = join(buildTarget, "debug/hachimi-desktop.exe");
const manifestPath = join(buildTarget, "ci-build.json");

function digest(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

export function verifyDesktopE2eBuild(executable = application, manifest = manifestPath) {
  const record = JSON.parse(readFileSync(manifest, "utf8"));
  if (
    record.profile !== "debug" ||
    record.feature !== "desktop-e2e" ||
    record.sha256 !== digest(executable)
  ) {
    throw new Error("The prebuilt Desktop E2E executable does not match its build manifest");
  }
}

export function buildDesktopE2e(environment = process.env) {
  const buildEnvironment = {
    ...environment,
    CARGO_NET_OFFLINE: "true",
    CARGO_TARGET_DIR: buildTarget,
    TAURI_CONFIG: JSON.stringify({ build: { devUrl: null } }),
  };
  const checked = (args) => {
    const result = spawnSync(process.execPath, args, {
      cwd: root,
      env: buildEnvironment,
      stdio: "inherit",
      windowsHide: true,
    });
    if (result.error) throw result.error;
    if (result.status !== 0) throw new Error(`Desktop E2E build failed with ${result.status}`);
  };
  checked(["scripts/prepare-workspace-worker.mjs", "dev"]);
  const corepackCli = join(dirname(process.execPath), "node_modules/corepack/dist/corepack.js");
  checked([corepackCli, "pnpm", "--dir", "apps/desktop/web", "build"]);
  // Only the disposable E2E PDB is removed; source and shared Cargo targets are preserved.
  rmSync(join(buildTarget, "debug/deps/hachimi_desktop.pdb"), { force: true });
  checked([
    "scripts/run-with-rust.mjs",
    "cargo",
    "build",
    "--offline",
    "-p",
    "hachimi-desktop",
    "--features",
    "desktop-e2e",
  ]);
  writeFileSync(
    manifestPath,
    JSON.stringify(
      {
        profile: "debug",
        feature: "desktop-e2e",
        sha256: digest(application),
      },
      null,
      2,
    ) + "\n",
  );
}

if (process.argv[1] && pathToFileURL(resolve(process.argv[1])).href === import.meta.url) {
  buildDesktopE2e();
}

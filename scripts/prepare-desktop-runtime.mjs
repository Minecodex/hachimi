import { createHash } from "node:crypto";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  writeFileSync,
} from "node:fs";
import { delimiter, dirname, isAbsolute, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

// Prepares platform-specific desktop runtime assets before `tauri dev` / `tauri build`.
// Windows stages the CEF embedded browser runtime via scripts/build-cef-host.ps1;
// macOS stages it natively below (docs/mac-plan/phase-4-cef-embedded-browser.md).

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

if (process.platform === "darwin") {
  stageMacCefRuntime();
  process.exit(0);
}

// tauri.conf.json maps target/cef-bundle/ as a bundle resource; the directory
// must exist even though the CEF runtime is not staged on this platform.
const bundleDirectory = join(workspaceRoot, "target", "cef-bundle");
mkdirSync(bundleDirectory, { recursive: true });
const keep = join(bundleDirectory, ".gitkeep");
if (!existsSync(keep)) writeFileSync(keep, "");
console.log(
  "CEF embedded browser runtime is not staged on this platform " +
    "(see docs/mac-plan/phase-4-cef-embedded-browser.md).",
);

function sha256File(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

function run(command, arguments_, environment) {
  const result = spawnSync(command, arguments_, {
    cwd: workspaceRoot,
    stdio: "inherit",
    env: environment,
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(`${command} ${arguments_.join(" ")} failed with exit code ${result.status}`);
  }
}

// Mirrors scripts/build-cef-host.ps1: pinned CMake + Ninja toolchain, a cargo
// build of the CEF host library (whose build script downloads the pinned CEF
// archive), an archive checksum assertion, then the .app bundler.
function stageMacCefRuntime() {
  const toolingRoot = join(workspaceRoot, "target", "tooling");
  const downloadRoot = join(toolingRoot, "downloads");
  const ninjaVersion = "1.13.1";
  const cmakeVersion = "3.31.8";
  const tools = [
    {
      name: "ninja",
      url: `https://github.com/ninja-build/ninja/releases/download/v${ninjaVersion}/ninja-mac.zip`,
      archive: join(downloadRoot, `ninja-mac-${ninjaVersion}.zip`),
      sha256: "da7797794153629aca5570ef7c813342d0be214ba84632af886856e8f0063dd9",
      destination: join(toolingRoot, "ninja"),
      executable: join(toolingRoot, "ninja", "ninja"),
      archiveKind: "zip",
    },
    {
      name: "cmake",
      url: `https://github.com/Kitware/CMake/releases/download/v${cmakeVersion}/cmake-${cmakeVersion}-macos-universal.tar.gz`,
      archive: join(downloadRoot, `cmake-${cmakeVersion}-macos-universal.tar.gz`),
      sha256: "d1449f969c54d5c00886d5b643340d493dfb3c81cb39ee29b35453395c11ebf7",
      destination: join(toolingRoot, "cmake"),
      executable: join(
        toolingRoot,
        "cmake",
        `cmake-${cmakeVersion}-macos-universal`,
        "CMake.app",
        "Contents",
        "bin",
        "cmake",
      ),
      archiveKind: "tar.gz",
    },
  ];
  mkdirSync(downloadRoot, { recursive: true });
  for (const tool of tools) {
    if (!existsSync(tool.archive)) {
      console.log(`Downloading ${tool.name} from ${tool.url}`);
      run("curl", ["-fSL", "--retry", "3", "-o", tool.archive, tool.url], process.env);
    }
    const actualHash = sha256File(tool.archive);
    if (actualHash !== tool.sha256) {
      throw new Error(
        `${tool.name} archive checksum mismatch: expected ${tool.sha256}, got ${actualHash}`,
      );
    }
    if (!existsSync(tool.executable)) {
      mkdirSync(tool.destination, { recursive: true });
      if (tool.archiveKind === "zip") {
        run("unzip", ["-o", tool.archive, "-d", tool.destination], process.env);
      } else {
        run("tar", ["-xzf", tool.archive, "-C", tool.destination], process.env);
      }
      chmodSync(tool.executable, 0o755);
    }
  }

  const cargoTargetRoot = process.env.CARGO_TARGET_DIR
    ? isAbsolute(process.env.CARGO_TARGET_DIR)
      ? process.env.CARGO_TARGET_DIR
      : resolve(workspaceRoot, process.env.CARGO_TARGET_DIR)
    : join(workspaceRoot, "target");
  const profileDirectoryName = mode === "release" ? "release" : "debug";
  const profileDirectory = join(cargoTargetRoot, profileDirectoryName);
  const profileArguments = mode === "release" ? ["--release"] : [];
  const cargoEnvironment = {
    ...process.env,
    CMAKE: tools[1].executable,
    PATH: `${tools[0].destination}${delimiter}${process.env.PATH ?? ""}`,
  };
  const runWithRust = join(scriptsDirectory, "run-with-rust.mjs");

  // Helpers initialize the CEF sandbox via libcef_sandbox.dylib in main.rs
  // (cef::sandbox::Sandbox), so the default `sandbox` feature stays enabled.
  // The mac bundler embeds the real host executable, so build the bin itself
  // (the Windows flow bundles the cdylib and only needs --lib).
  run(
    process.execPath,
    [
      runWithRust,
      "cargo",
      "build",
      "-p",
      "hachimi-cef-host",
      "--bin",
      "hachimi-cef-host",
      ...profileArguments,
    ],
    cargoEnvironment,
  );

  const archiveName =
    "cef_binary_151.3.14+g5d67476+chromium-151.0.7922.72_macosarm64_minimal.tar.bz2";
  const archiveSha256 = "e3d268c88c612548f679aa454457e415d41c796bc5996f592b8263749c9681fd";
  const buildRoot = join(profileDirectory, "build");
  const archivePath = existsSync(buildRoot)
    ? readdirSync(buildRoot)
        .map((entry) => join(buildRoot, entry, "out", archiveName))
        .find((candidate) => existsSync(candidate))
    : undefined;
  if (!archivePath) {
    throw new Error("CEF archive was not retained by the pinned cef build");
  }
  const actualCefHash = sha256File(archivePath);
  if (actualCefHash !== archiveSha256) {
    throw new Error(
      `CEF archive checksum mismatch: expected ${archiveSha256}, got ${actualCefHash}`,
    );
  }

  const bundlePath = join(workspaceRoot, "target", "cef-bundle");
  run(
    process.execPath,
    [
      runWithRust,
      "cargo",
      "run",
      ...profileArguments,
      "-p",
      "hachimi-cef-host",
      "--bin",
      "bundle-hachimi-cef-host",
      "--",
      bundlePath,
      profileDirectory,
    ],
    cargoEnvironment,
  );
  console.log(
    "Staged macOS CEF embedded browser runtime at target/cef-bundle/hachimi-cef-host.app",
  );
}

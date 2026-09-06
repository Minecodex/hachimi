import { spawnSync } from "node:child_process";
import {
  cpSync,
  mkdirSync,
  mkdtempSync,
  openSync,
  readFileSync,
  readdirSync,
  readSync,
  rmSync,
  statSync,
  symlinkSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

const workspaceRoot = resolve(fileURLToPath(new URL("../..", import.meta.url)));
const tauriConfig = "apps/desktop/src-tauri/tauri.conf.json";

export function deriveMsiVersion(sourceVersion) {
  const match = /^(\d+)\.(\d+)\.(\d+)(?:-alpha\.(\d+))?$/.exec(sourceVersion);
  if (!match) throw new Error(`release_msi_version_unsupported:${sourceVersion}`);
  if (match[4] === undefined) return sourceVersion;
  const prerelease = Number(match[4]);
  if (!Number.isSafeInteger(prerelease) || prerelease > 65_535) {
    throw new Error(`release_msi_prerelease_out_of_range:${match[4]}`);
  }
  return `${match[1]}.${match[2]}.${match[3]}-${prerelease}`;
}

function runTauri(arguments_) {
  const result = spawnSync(
    process.execPath,
    ["scripts/run-with-rust.mjs", "tauri", ...arguments_],
    {
      cwd: workspaceRoot,
      env: process.env,
      stdio: "inherit",
      windowsHide: true,
    },
  );
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(`release_installer_build_failed:${arguments_[1]}:${result.status}`);
  }
}

function buildInstallers() {
  const sourceVersion = JSON.parse(
    readFileSync(resolve(workspaceRoot, "package.json"), "utf8"),
  ).version;
  const msiVersion = deriveMsiVersion(sourceVersion);
  const bundleRoot = resolve(workspaceRoot, "target/release/bundle");
  for (const kind of ["nsis", "msi"]) {
    const output = resolve(bundleRoot, kind);
    if (!output.startsWith(`${bundleRoot}${sep}`)) {
      throw new Error(`release_installer_output_escaped:${output}`);
    }
    mkdirSync(output, { recursive: true });
    for (const entry of readdirSync(output)) {
      rmSync(resolve(output, entry), { recursive: true, force: true });
    }
  }
  process.stdout.write(`Building NSIS with source version ${sourceVersion}.\n`);
  runTauri(["build", "--bundles", "nsis", "--config", tauriConfig]);
  process.stdout.write(`Building MSI with numeric prerelease overlay ${msiVersion}.\n`);
  runTauri([
    "build",
    "--bundles",
    "msi",
    "--config",
    tauriConfig,
    "--config",
    JSON.stringify({ version: msiVersion }),
  ]);
}

// macOS dev-channel installer (方案 A, docs/mac-plan/phase-5): an ad-hoc
// signed .app + dmg. No Apple Developer ID, no notarization — users open it
// once via right-click → Open (documented in the release notes).
function isMachO(path) {
  try {
    if (!statSync(path).isFile()) return false;
    const fd = openSync(path, "r");
    const magic = Buffer.alloc(4);
    readSync(fd, magic, 0, 4, 0);
    return (
      magic.readUInt32BE(0) === 0xfeedface || // MH_MAGIC
      magic.readUInt32BE(0) === 0xfeedfacf || // MH_MAGIC_64
      magic.readUInt32BE(0) === 0xcafebabe // FAT_MAGIC
    );
  } catch {
    return false;
  }
}

function codesignAdHoc(target) {
  const result = spawnSync("codesign", ["--force", "--sign", "-", target], {
    cwd: workspaceRoot,
    encoding: "utf8",
  });
  if (result.status !== 0) {
    throw new Error(`codesign failed for ${target}: ${result.stderr?.trim()}`);
  }
}

// Ad-hoc re-sign inside out: tauri copies resources into the bundle after
// link time, which leaves stale resource seals on every Mach-O it shipped
// (codesign --verify fails with "code has no resources…"). Nested code must
// be signed before its enclosing bundle.
function adHocResignMacApp(appPath) {
  const resources = join(appPath, "Contents", "Resources");
  const cefApp = join(resources, "cef-runtime", "hachimi-cef-host.app");
  const frameworksDir = join(cefApp, "Contents", "Frameworks");
  const framework = join(frameworksDir, "Chromium Embedded Framework.framework");
  for (const entry of readdirSync(join(framework, "Libraries"))) {
    const dylib = join(framework, "Libraries", entry);
    if (entry.endsWith(".dylib")) codesignAdHoc(dylib);
  }
  codesignAdHoc(framework);
  for (const helper of readdirSync(frameworksDir).filter((n) => n.endsWith(".app"))) {
    const helperApp = join(frameworksDir, helper);
    for (const exe of readdirSync(join(helperApp, "Contents", "MacOS"))) {
      codesignAdHoc(join(helperApp, "Contents", "MacOS", exe));
    }
    codesignAdHoc(helperApp);
  }
  codesignAdHoc(join(cefApp, "Contents", "MacOS", "hachimi-cef-host"));
  codesignAdHoc(cefApp);
  for (const entry of readdirSync(join(resources, "sherpa-onnx"))) {
    if (entry.endsWith(".dylib")) codesignAdHoc(join(resources, "sherpa-onnx", entry));
  }
  for (const entry of readdirSync(join(resources, "internal-runtime"))) {
    const sidecar = join(resources, "internal-runtime", entry);
    if (isMachO(sidecar)) codesignAdHoc(sidecar);
  }
  codesignAdHoc(appPath);
}

function buildMacInstaller() {
  const { version } = JSON.parse(readFileSync(resolve(workspaceRoot, "package.json"), "utf8"));
  // The .app is ~1.4GiB and dmg staging duplicates it; fail early with a
  // clear error instead of shipping a truncated bundle when disk pressure
  // hits (observed on a full disk: corrupted copies panic at startup).
  run("node", ["scripts/release/check-disk-space.mjs", "8"], process.env);
  const bundleRoot = resolve(workspaceRoot, "target/release/bundle");
  for (const kind of ["macos", "dmg"]) {
    const output = resolve(bundleRoot, kind);
    if (!output.startsWith(`${bundleRoot}${sep}`)) {
      throw new Error(`release_installer_output_escaped:${output}`);
    }
    rmSync(output, { recursive: true, force: true });
  }
  process.stdout.write("Building macOS app bundle (ad-hoc dev channel).\n");
  // Build only the .app here; the dmg is created below from the re-signed
  // bundle so it never ships a stale signature.
  runTauri(["build", "--bundles", "app", "--config", tauriConfig]);

  const appPath = join(bundleRoot, "macos", "Hachimi.app");
  adHocResignMacApp(appPath);
  const verify = spawnSync("codesign", ["--verify", "--deep", "--strict", appPath], {
    encoding: "utf8",
  });
  if (verify.status !== 0) {
    throw new Error(`release_installer_signature_invalid:${verify.stderr?.trim()}`);
  }

  const dmgDirectory = join(bundleRoot, "dmg");
  mkdirSync(dmgDirectory, { recursive: true });
  const dmgPath = join(dmgDirectory, `Hachimi_${version}_aarch64.dmg`);
  rmSync(dmgPath, { force: true });
  const staging = mkdtempSync(join(tmpdir(), "hachimi-dmg-"));
  try {
    copyAppAndLinkApplications(staging, appPath);
    run(
      "hdiutil",
      ["create", "-volname", "Hachimi", "-srcfolder", staging, "-ov", "-format", "UDZO", dmgPath],
      process.env,
    );
  } finally {
    rmSync(staging, { recursive: true, force: true });
  }
  process.stdout.write(`macOS dev installer ready: ${dmgPath}\n`);
}

function copyAppAndLinkApplications(staging, appPath) {
  cpSync(appPath, join(staging, "Hachimi.app"), { recursive: true, verbatim: true });
  symlinkSync("/Applications", join(staging, "Applications"));
}

function run(command, arguments_, environment) {
  const result = spawnSync(command, arguments_, {
    cwd: workspaceRoot,
    env: environment,
    stdio: "inherit",
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(`${command} ${arguments_[0]} failed with exit code ${result.status}`);
  }
}

const isCli = process.argv[1] && fileURLToPath(import.meta.url) === resolve(process.argv[1]);
if (isCli) {
  if (process.platform === "darwin") {
    buildMacInstaller();
  } else if (process.platform === "win32") {
    buildInstallers();
  } else {
    throw new Error(`release_installer_platform_unsupported:${process.platform}`);
  }
}

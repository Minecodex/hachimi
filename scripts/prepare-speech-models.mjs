import { createHash, randomUUID } from "node:crypto";
import {
  cpSync,
  copyFileSync,
  createReadStream,
  existsSync,
  mkdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { dirname, isAbsolute, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const scriptsDirectory = dirname(fileURLToPath(import.meta.url));
const workspaceRoot = resolve(scriptsDirectory, "..");
const cacheRoot = join(workspaceRoot, "target", "speech-model-cache");
const destinationRoot = join(
  workspaceRoot,
  "apps",
  "desktop",
  "src-tauri",
  "resources",
  "ai-models",
);

const ARCHIVES = {
  vits: {
    name: "vits-melo-tts-zh_en.tar.bz2",
    url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/vits-melo-tts-zh_en.tar.bz2",
    sha256: "e58351ed7149f290a54534538badd4077cdbe6fddc964b24d0bee870415d1514",
  },
  senseVoice: {
    name: "sensevoice-small-int8.tar.bz2",
    url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2025-09-09.tar.bz2",
    sha256: "7305f7905bfcf77fa0b39388a313f3da35c68d971661a65475b56fb2162c8e63",
  },
};

async function sha256File(path) {
  const hash = createHash("sha256");
  for await (const chunk of createReadStream(path)) hash.update(chunk);
  return hash.digest("hex");
}

function resolveInside(root, relativePath) {
  const path = resolve(root, relativePath);
  const rel = relative(root, path);
  if (!rel || rel === ".." || rel.startsWith(`..${sep}`) || isAbsolute(rel)) {
    throw new Error(`Path must stay inside ${root}: ${relativePath}`);
  }
  return path;
}

async function verifiedArchive({ name, url, sha256 }) {
  mkdirSync(cacheRoot, { recursive: true });
  const archive = join(cacheRoot, name);
  if (existsSync(archive)) {
    if ((await sha256File(archive)) === sha256) return archive;
    rmSync(archive);
  }
  console.log(`Downloading ${url}`);
  const response = await fetch(url, { redirect: "follow" });
  if (!response.ok) throw new Error(`Download failed for ${name}: HTTP ${response.status}`);
  const bytes = Buffer.from(await response.arrayBuffer());
  const actual = createHash("sha256").update(bytes).digest("hex");
  if (actual !== sha256) {
    throw new Error(`SHA-256 mismatch for ${name}. Expected ${sha256}, got ${actual}.`);
  }
  writeFileSync(archive, bytes);
  return archive;
}

function extract(archive, buildRoot) {
  const result = spawnSync("tar", ["-xf", archive, "-C", buildRoot], { stdio: "inherit" });
  if (result.status !== 0) {
    throw new Error(`Failed to extract ${archive} (tar exited with ${result.status}).`);
  }
}

function copyRequired(sourceRoot, names, destination) {
  mkdirSync(destination, { recursive: true });
  for (const name of names) {
    const source = join(sourceRoot, name);
    if (!existsSync(source)) throw new Error(`Model archive is missing ${name}.`);
    cpSync(source, join(destination, name), { recursive: true });
  }
}

const buildRoot = join(workspaceRoot, "target", `speech-model-build-${randomUUID()}`);
mkdirSync(buildRoot, { recursive: true });

try {
  const vitsArchive = await verifiedArchive(ARCHIVES.vits);
  const senseVoiceArchive = await verifiedArchive(ARCHIVES.senseVoice);

  extract(vitsArchive, buildRoot);
  // Only replace generated artifacts. Documentation and notices in the model
  // directory are repository-owned and must survive a repair/download cycle.
  const vitsDestination = resolveInside(
    destinationRoot,
    "text-to-speech/vits-melo-zh-en/vits-melo-tts-zh_en",
  );
  const vitsModelRoot = dirname(vitsDestination);
  const legacyVitsRoot = resolveInside(destinationRoot, "text-to-speech/vits-chaowen-int8");
  rmSync(legacyVitsRoot, { recursive: true, force: true });
  rmSync(vitsDestination, { recursive: true, force: true });
  rmSync(join(vitsModelRoot, "manifest.json"), { force: true });
  copyRequired(
    join(buildRoot, "vits-melo-tts-zh_en"),
    [
      "model.onnx",
      "tokens.txt",
      "lexicon.txt",
      "dict",
      "new_heteronym.fst",
      "phone.fst",
      "date.fst",
      "number.fst",
      "README.md",
      "LICENSE",
    ],
    vitsDestination,
  );
  copyFileSync(
    join(scriptsDirectory, "model-manifests", "vits-melo-zh-en.json"),
    join(vitsModelRoot, "manifest.json"),
  );

  extract(senseVoiceArchive, buildRoot);
  // Keep tracked README/license files and replace only generated model data.
  const senseVoiceDestination = resolveInside(destinationRoot, "speech-to-text/sensevoice-small");
  mkdirSync(senseVoiceDestination, { recursive: true });
  for (const name of ["model.int8.onnx", "tokens.txt", "manifest.json"]) {
    rmSync(join(senseVoiceDestination, name), { force: true });
  }
  copyRequired(
    join(buildRoot, "sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2025-09-09"),
    ["model.int8.onnx", "tokens.txt"],
    senseVoiceDestination,
  );
  copyFileSync(
    join(scriptsDirectory, "model-manifests", "sensevoice-small.json"),
    join(senseVoiceDestination, "manifest.json"),
  );
} finally {
  rmSync(buildRoot, { recursive: true, force: true });
}

console.log(
  "Prepared the MIT-licensed bilingual MeloTTS voice and bundled SenseVoice-Small under resources/ai-models.",
);

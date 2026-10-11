import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { verifyDesktopE2eBuild } from "./build.mjs";

test("prebuilt E2E execution requires the matching debug executable", () => {
  const root = mkdtempSync(join(tmpdir(), "hachimi-e2e-build-test-"));
  try {
    const executable = join(root, "fixture.exe");
    const manifest = join(root, "ci-build.json");
    const bytes = "synthetic executable fixture";
    writeFileSync(executable, bytes);
    const record = {
      profile: "debug",
      feature: "desktop-e2e",
      sha256: createHash("sha256").update(bytes).digest("hex"),
    };
    writeFileSync(manifest, JSON.stringify(record));
    assert.doesNotThrow(() => verifyDesktopE2eBuild(executable, manifest));
    writeFileSync(executable, "changed executable");
    assert.throws(() => verifyDesktopE2eBuild(executable, manifest), /does not match/);
    writeFileSync(executable, bytes);
    writeFileSync(manifest, JSON.stringify({ ...record, profile: "release" }));
    assert.throws(() => verifyDesktopE2eBuild(executable, manifest), /does not match/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

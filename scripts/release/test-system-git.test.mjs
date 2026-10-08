import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, join } from "node:path";
import test from "node:test";
import { resolveTestGit } from "../test-system-git.mjs";

test("Cargo tests receive the canonical system Git from absolute PATH entries", (t) => {
  const root = mkdtempSync(join(tmpdir(), "hachimi-test-git-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const bin = join(root, "bin");
  mkdirSync(bin);
  const executable = join(bin, process.platform === "win32" ? "git.exe" : "git");
  writeFileSync(executable, "test executable");
  assert.equal(resolveTestGit({ PATH: `relative${delimiter}${bin}` }), realpathSync(executable));
});

test("An explicit unavailable Git is rejected even if PATH has candidates", () => {
  assert.throws(() => resolveTestGit({ HACHIMI_GIT_EXECUTABLE: "relative/git" }), /absolute/);
});

test("Empty and relative PATH entries never become the Git test override", () => {
  assert.throws(() => resolveTestGit({ PATH: `${delimiter}relative` }), /absolute PATH/);
});

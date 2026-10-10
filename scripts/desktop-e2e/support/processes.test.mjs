import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { copyFileSync, existsSync, mkdtempSync, readFileSync, realpathSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import test from "node:test";

import { cleanupExecutableProcesses, terminateProcessTree } from "./processes.mjs";

function alive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    if (error.code === "ESRCH") return false;
    throw error;
  }
}

async function waitUntil(condition, message) {
  const deadline = Date.now() + 10_000;
  while (!(await condition())) {
    assert.ok(Date.now() < deadline, message);
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
}

test(
  "exact application cleanup terminates its native descendants",
  {
    skip: process.platform !== "win32",
  },
  async (t) => {
    const parentDirectory = realpathSync(tmpdir());
    const root = realpathSync(mkdtempSync(join(parentDirectory, "hachimi-process-tree-test-")));
    const executable = join(root, "fixture-node.exe");
    const marker = join(root, "descendant.pid");
    copyFileSync(process.execPath, executable);
    let descendant;
    const application = spawn(
      executable,
      [
        "-e",
        String.raw`
    const { spawn } = require('node:child_process');
    spawn(process.env.HACHIMI_PROCESS_TREE_CHILD_NODE, ['-e',
      'require("node:fs").writeFileSync(process.env.HACHIMI_PROCESS_TREE_MARKER, String(process.pid)); setInterval(() => {}, 1000);'],
      { stdio: 'ignore', windowsHide: true, detached: true });
    setInterval(() => {}, 1000);
  `,
      ],
      {
        env: {
          ...process.env,
          HACHIMI_PROCESS_TREE_MARKER: marker,
          HACHIMI_PROCESS_TREE_CHILD_NODE: process.execPath,
        },
        stdio: "ignore",
        windowsHide: true,
      },
    );
    t.after(() => {
      terminateProcessTree(application.pid);
      if (descendant && alive(descendant)) terminateProcessTree(descendant);
      assert.equal(dirname(root), parentDirectory);
      assert.ok(root.startsWith(join(parentDirectory, "hachimi-process-tree-test-")));
      rmSync(root, { recursive: true, force: true });
    });
    await waitUntil(() => existsSync(marker), "Native descendant did not start");
    descendant = Number(readFileSync(marker, "utf8"));
    assert.ok(Number.isInteger(descendant) && descendant > 0);
    assert.ok(alive(descendant));
    cleanupExecutableProcesses(executable);
    await waitUntil(
      () => !alive(descendant),
      "Owned native descendant survived application cleanup",
    );
  },
);

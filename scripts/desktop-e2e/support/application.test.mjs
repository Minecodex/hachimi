import assert from "node:assert/strict";
import { mkdtempSync, realpathSync, rmSync } from "node:fs";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import test from "node:test";

import { launchAutomationApplication } from "./application.mjs";
import { terminateProcessTree } from "./processes.mjs";

function fixtureRoot(t) {
  const parent = realpathSync(tmpdir());
  const root = realpathSync(mkdtempSync(join(parent, "hachimi-automation-test-")));
  t.after(() => {
    assert.equal(dirname(root), parent);
    assert.ok(root.startsWith(join(parent, "hachimi-automation-test-")));
    rmSync(root, { recursive: true, force: true });
  });
  return root;
}

const fixtureApplication = `
  const http = require('node:http');
  const port = Number(process.env.WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS.match(/--remote-debugging-port=(\\d+)/)[1]);
  const server = http.createServer((request, response) => {
    response.setHeader('content-type', 'application/json');
    response.end(JSON.stringify({
      webSocketDebuggerUrl: 'ws://127.0.0.1:' + port + '/devtools/browser/fixture',
      profile: process.env.WEBVIEW2_USER_DATA_FOLDER,
      automation: process.env.TAURI_WEBVIEW_AUTOMATION,
    }));
  });
  setTimeout(() => server.listen(port, '127.0.0.1'), 100);
`;

test("automation waits for the application's endpoint and uses fresh profiles across launches", async (t) => {
  const root = fixtureRoot(t);
  const applications = [];
  try {
    for (let index = 0; index < 2; index += 1) {
      const application = await launchAutomationApplication(process.execPath, root, [
        "-e",
        fixtureApplication,
      ]);
      applications.push(application);
      const response = await fetch(`http://${application.debuggerAddress}/json/version`);
      const state = await response.json();
      assert.equal(state.profile, application.profile);
      assert.equal(state.automation, "true");
    }
    assert.notEqual(applications[0].profile, applications[1].profile);
    assert.notEqual(applications[0].debuggerAddress, applications[1].debuggerAddress);
  } finally {
    for (const application of applications) terminateProcessTree(application.pid);
  }
});

test("automation reports application exits before a DevTools endpoint is available", async (t) => {
  await assert.rejects(
    launchAutomationApplication(process.execPath, fixtureRoot(t), ["-e", "process.exit(23)"]),
    /exited with 23/,
  );
});

test("new automation launches leave an occupied preceding endpoint untouched", async (t) => {
  const preceding = createServer();
  await new Promise((resolve, reject) => {
    preceding.once("error", reject);
    preceding.listen(0, "127.0.0.1", resolve);
  });
  const priorValue = process.env.HACHIMI_DESKTOP_E2E_DEBUG_PORT;
  const precedingPort = preceding.address().port;
  process.env.HACHIMI_DESKTOP_E2E_DEBUG_PORT = String(precedingPort);
  let application;
  try {
    application = await launchAutomationApplication(process.execPath, fixtureRoot(t), [
      "-e",
      fixtureApplication,
    ]);
    assert.notEqual(application.debuggerAddress, `127.0.0.1:${precedingPort}`);
    assert.equal(preceding.listening, true);
  } finally {
    if (application) terminateProcessTree(application.pid);
    await new Promise((resolve) => preceding.close(resolve));
    if (priorValue === undefined) delete process.env.HACHIMI_DESKTOP_E2E_DEBUG_PORT;
    else process.env.HACHIMI_DESKTOP_E2E_DEBUG_PORT = priorValue;
  }
});

test("automation reports an unavailable executable without waiting for readiness", async (t) => {
  const root = fixtureRoot(t);
  await assert.rejects(launchAutomationApplication(join(root, "missing.exe"), root), /ENOENT/);
});

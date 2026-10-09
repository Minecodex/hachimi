import { mkdirSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";

/* global document */

import { cleanupExecutableProcesses } from "./support/processes.mjs";
import {
  attachedWebviewCapabilities,
  launchAutomationApplication,
} from "./support/application.mjs";

const artifacts =
  process.env.HACHIMI_DESKTOP_E2E_ARTIFACTS ?? resolve("target/desktop-e2e-artifacts");
mkdirSync(artifacts, { recursive: true });
const requestedSpec = process.env.HACHIMI_DESKTOP_E2E_SPEC;
const attachToApplication = process.env.HACHIMI_DESKTOP_E2E_ATTACH === "1";

export const config = {
  runner: "local",
  hostname: "127.0.0.1",
  port: 4444,
  path: "/",
  specs: requestedSpec
    ? [resolve(requestedSpec)]
    : [
        resolve("scripts/desktop-e2e/specs/workbench-core.e2e.mjs"),
        resolve("scripts/desktop-e2e/specs/resume-rejoin.e2e.mjs"),
        resolve("scripts/desktop-e2e/specs/agent-tools.e2e.mjs"),
        resolve("scripts/desktop-e2e/specs/extensions-settings.e2e.mjs"),
        resolve("scripts/desktop-e2e/specs/host-integrations.e2e.mjs"),
        resolve("scripts/desktop-e2e/specs/task-center.e2e.mjs"),
        resolve("scripts/desktop-e2e/specs/avatar-motion-v5.e2e.mjs"),
      ],
  maxInstances: 1,
  capabilities: [
    attachToApplication
      ? {
          maxInstances: 1,
          ...attachedWebviewCapabilities(`127.0.0.1:${process.env.HACHIMI_DESKTOP_E2E_DEBUG_PORT}`),
        }
      : {
          maxInstances: 1,
          "tauri:options": {
            application: process.env.HACHIMI_DESKTOP_E2E_APP,
            webviewOptions: {
              browserExecutableFolder: process.env.WEBVIEW2_BROWSER_EXECUTABLE_FOLDER,
              userDataFolder: process.env.HACHIMI_DESKTOP_E2E_WEBVIEW_DATA,
              additionalBrowserArguments: ["--remote-debugging-port=0"],
            },
          },
        },
  ],
  logLevel: "warn",
  framework: "mocha",
  reporters: ["spec"],
  mochaOpts: {
    // The release lifecycle tests intentionally cross multiple fresh
    // checkout-bound Host processes, application restarts and ConPTY/MCP
    // boundaries. A single assertion keeps its own much smaller timeout; this
    // ceiling only prevents Mocha from aborting a progressing end-to-end flow.
    timeout: 600_000,
    grep: process.env.HACHIMI_DESKTOP_E2E_GREP || undefined,
  },
  connectionRetryCount: 0,
  beforeSession: async (_config, capabilities) => {
    if (!attachToApplication) return;
    const application = await launchAutomationApplication(
      process.env.HACHIMI_DESKTOP_E2E_APP,
      process.env.HACHIMI_DESKTOP_E2E_WEBVIEW_DATA,
    );
    Object.assign(capabilities, attachedWebviewCapabilities(application.debuggerAddress));
  },
  afterTest: async (_test, _context, result) => {
    if (!result.passed) {
      const safeName = `failure-${Date.now()}`;
      try {
        await browser.saveScreenshot(resolve(artifacts, `${safeName}.png`));
        writeFileSync(resolve(artifacts, `${safeName}.html`), await browser.getPageSource());
        const snapshot = await browser.executeAsync((done) => {
          const runId = document
            .querySelector('[data-testid="workbench-session-timeline"]')
            ?.getAttribute("data-run-id");
          if (!runId) return done(null);
          const invoke = window.__TAURI_INTERNALS__.invoke;
          invoke("list_workbench_sessions", { projectId: null })
            .then((sessions) => {
              const session = sessions.find((entry) => entry.latestRun?.id === runId);
              return session
                ? invoke("get_workbench_session", { sessionId: session.session.id })
                : null;
            })
            .then(done, (error) => done({ diagnosticError: String(error) }));
        });
        if (snapshot) {
          writeFileSync(resolve(artifacts, `${safeName}.json`), JSON.stringify(snapshot, null, 2));
          const childSessionIds = [
            ...new Set((snapshot.agentTasks ?? []).map((task) => task.childSessionId)),
          ];
          for (const [index, sessionId] of childSessionIds.entries()) {
            try {
              const child = await browser.executeAsync((id, done) => {
                window.__TAURI_INTERNALS__
                  .invoke("get_workbench_session", { sessionId: id })
                  .then(done, (error) => done({ diagnosticError: String(error) }));
              }, sessionId);
              writeFileSync(
                resolve(artifacts, `${safeName}-child-${index}.json`),
                JSON.stringify(child, null, 2),
              );
            } catch {
              /* Retain the original failure and parent snapshot. */
            }
          }
        }
      } catch {
        // Preserve the original failure when a restart already invalidated the
        // WebDriver session and no screenshot can be captured.
      }
    }
  },
  afterSession: () => {
    cleanupExecutableProcesses(process.env.HACHIMI_DESKTOP_E2E_APP);
  },
};

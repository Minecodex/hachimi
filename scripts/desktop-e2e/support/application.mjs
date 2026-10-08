import { spawn } from "node:child_process";
import { createWriteStream, mkdirSync } from "node:fs";
import { createServer } from "node:net";
import { join } from "node:path";

import { terminateProcessTree } from "./processes.mjs";

let launchSequence = 0;

export async function launchAutomationApplication(application, profileRoot, args = []) {
  if (!application || !profileRoot) throw new Error("Desktop E2E application paths are missing");
  const reservation = createServer();
  await new Promise((resolve, reject) => {
    reservation.once("error", reject);
    const requestedPort = Number(process.env.HACHIMI_DESKTOP_E2E_DEBUG_PORT || 0);
    reservation.listen(requestedPort, "127.0.0.1", resolve);
  });
  const port = reservation.address().port;
  await new Promise((resolve) => reservation.close(resolve));
  const profile = join(profileRoot, `session-${process.pid}-${++launchSequence}`);
  mkdirSync(profile, { recursive: true });
  const child = spawn(application, args, {
    env: {
      ...process.env,
      TAURI_WEBVIEW_AUTOMATION: "true",
      WEBVIEW2_USER_DATA_FOLDER: profile,
      WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port} --remote-debugging-address=127.0.0.1`,
    },
    stdio: ["ignore", "pipe", "pipe"],
    windowsHide: true,
  });
  const artifacts = process.env.HACHIMI_DESKTOP_E2E_ARTIFACTS;
  if (artifacts) {
    const log = createWriteStream(join(artifacts, "application-launch.log"), { flags: "a" });
    child.stdout.pipe(log, { end: false });
    child.stderr.pipe(log, { end: false });
    child.once("exit", () => log.end());
  } else {
    child.stdout.resume();
    child.stderr.resume();
  }
  let launchError;
  child.once("error", (error) => {
    launchError = error;
  });
  const debuggerAddress = `127.0.0.1:${port}`;
  const deadline = Date.now() + 45_000;
  try {
    while (Date.now() < deadline) {
      if (launchError) throw launchError;
      if (child.exitCode !== null)
        throw new Error(`Desktop E2E application exited with ${child.exitCode}`);
      try {
        const response = await fetch(`http://${debuggerAddress}/json/version`, {
          signal: AbortSignal.timeout(1000),
        });
        if (response.ok && (await response.json()).webSocketDebuggerUrl) {
          return { debuggerAddress, pid: child.pid, profile };
        }
      } catch {
        // The native app creates its WebViews after storage and runtime initialization.
      }
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
    throw new Error("Desktop E2E application did not expose its loopback DevTools endpoint");
  } catch (error) {
    terminateProcessTree(child.pid);
    throw error;
  }
}

export function attachedWebviewCapabilities(debuggerAddress) {
  return {
    browserName: "webview2",
    "wdio:enforceWebDriverClassic": true,
    "ms:edgeChromium": true,
    "ms:edgeOptions": { debuggerAddress },
  };
}

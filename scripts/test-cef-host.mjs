#!/usr/bin/env node
// macOS smoke test for the CEF embedded browser host in windowless (OSR) mode
// — the macOS equivalent of scripts/test-cef-host.ps1. Drives the bundled
// hachimi-cef-host.app over JSON-Lines IPC and asserts:
//   1. ready handshake
//   2. create_tab + OSR frame production at Retina scale (bounds 1600x1200
//      physical with scaleFactor 2 → 800x600 logical viewport)
//   3. native input injection (SendInput mouse click reaches the page)
//   4. was_resized semantics (set_bounds re-renders at the new size)
//   5. navigation error reporting
//   6. clean shutdown (exit code 0)
//
// Usage: node scripts/test-cef-host.mjs   (after `corepack pnpm cef:prepare`)

import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const hostExecutable = process.env.HACHIMI_CEF_HOST
  ? resolve(process.env.HACHIMI_CEF_HOST)
  : resolve(repoRoot, "target/cef-bundle/hachimi-cef-host.app/Contents/MacOS/hachimi-cef-host");

function fail(message) {
  console.error(`CEF-HOST-SMOKE-FAIL: ${message}`);
  process.exit(1);
}

if (process.platform !== "darwin") {
  fail("this smoke test only runs on macOS (Windows uses scripts/test-cef-host.ps1)");
}
if (!existsSync(hostExecutable)) {
  fail(`CEF host bundle is missing at ${hostExecutable}; run \`corepack pnpm cef:prepare\` first`);
}

const profile = mkdtempSync(join(tmpdir(), "hachimi-cef-smoke-"));
const framesDirectory = join(profile, "frames");

// Local marker page: solid color + click counter (loopback only; the host is
// launched with --no-proxy-server so a local system proxy cannot hijack it).
const markerPage = Buffer.from(
  "<html><head><script>window.__clicks=0;document.onclick=()=>{window.__clicks+=1;};</script></head>" +
    "<body style='margin:0;background:rgb(0,0,255)'></body></html>",
);
const server = createServer((request, response) => {
  response.writeHead(200, { "content-type": "text/html" });
  response.end(markerPage);
});
await new Promise((resolveListen) => server.listen(0, "127.0.0.1", resolveListen));
const port = server.address().port;

const environment = { ...process.env };
for (const key of Object.keys(environment)) {
  if (/_proxy$/i.test(key)) delete environment[key];
}
const child = spawn(
  hostExecutable,
  [
    "--hachimi-osr",
    "--hachimi-parent-hwnd=0",
    `--hachimi-profile-dir=${profile}`,
    `--hachimi-log-file=${join(profile, "cef.log")}`,
    "--no-proxy-server",
  ],
  // detached: own process group so cleanup can kill CEF helper processes too.
  { env: environment, stdio: ["pipe", "pipe", "inherit"], detached: true },
);

let stdoutBuffer = "";
const messages = [];
const waiters = [];
child.stdout.on("data", (chunk) => {
  stdoutBuffer += chunk;
  let newline;
  while ((newline = stdoutBuffer.indexOf("\n")) >= 0) {
    const line = stdoutBuffer.slice(0, newline);
    stdoutBuffer = stdoutBuffer.slice(newline + 1);
    if (!line.trim()) continue;
    try {
      const message = JSON.parse(line);
      messages.push(message);
      for (const waiter of [...waiters]) waiter(message);
    } catch {
      // Ignore non-JSON noise from CEF on stdout.
    }
  }
});

let nextRequestId = 1;
function send(command) {
  const envelope = { protocolVersion: 1, requestId: nextRequestId++, command };
  child.stdin.write(`${JSON.stringify(envelope)}\n`);
  return envelope.requestId;
}

function waitFor(predicate, description, timeoutMs = 20000) {
  const immediate = messages.find(predicate);
  if (immediate) return Promise.resolve(immediate);
  return new Promise((resolveWait, rejectWait) => {
    const timer = setTimeout(() => {
      const frames = messages
        .filter((message) => message.event?.kind === "frame_ready")
        .slice(-8)
        .map((message) => `${message.event.width}x${message.event.height}`);
      rejectWait(
        new Error(`timeout waiting for ${description}; recent frames=${frames.join(",")}`),
      );
    }, timeoutMs);
    waiters.push((message) => {
      if (predicate(message)) {
        clearTimeout(timer);
        resolveWait(message);
      }
    });
  });
}

const response = (requestId) => (message) =>
  message.kind === "response" && message.request_id === requestId;

async function expectAck(requestId, description) {
  const message = await waitFor(response(requestId), description);
  if (message.result?.Err) {
    throw new Error(`${description} was rejected: ${JSON.stringify(message.result.Err)}`);
  }
}

const frameReady = (tabId, width, height) => (message) =>
  message.kind === "event" &&
  message.event?.kind === "frame_ready" &&
  message.event?.tab_id === tabId &&
  (width === undefined || (message.event?.width === width && message.event?.height === height));

try {
  await waitFor(
    (message) => message.kind === "ready" && message.protocol_version === 1,
    "ready handshake",
  );

  await expectAck(
    send({
      kind: "create_tab",
      tab_id: "tab-smoke",
      url: `http://127.0.0.1:${port}/`,
      bounds: { x: 0, y: 0, width: 1600, height: 1200, scaleFactor: 2 },
      visible: true,
    }),
    "create_tab",
  );

  // Retina path: 1600x1200 physical bounds at scaleFactor 2 render an
  // 800x600 logical viewport whose frames come back at 1600x1200 pixels.
  await waitFor(frameReady("tab-smoke", 1600, 1200), "Retina-scale frame_ready");
  const frame = readFileSync(join(framesDirectory, "tab-smoke.bgra"));
  if (frame.length !== 1600 * 1200 * 4) {
    throw new Error(`frame size mismatch: ${frame.length} bytes`);
  }

  // Native input injection: a click at the logical viewport center must reach
  // the page's DOM handler (CefInputEvent → send_mouse_click_event).
  await expectAck(
    send({
      kind: "send_input",
      tab_id: "tab-smoke",
      event: { type: "mouse_move", x: 400, y: 300, modifiers: 0, leave: false },
    }),
    "mouse move",
  );
  for (const up of [false, true]) {
    await expectAck(
      send({
        kind: "send_input",
        tab_id: "tab-smoke",
        event: {
          type: "mouse_button",
          x: 400,
          y: 300,
          modifiers: 0,
          button: "left",
          up,
          click_count: 1,
        },
      }),
      "mouse click",
    );
  }
  const evaluation = await waitFor(
    response(
      send({
        kind: "dev_tools",
        tab_id: "tab-smoke",
        method: "Runtime.evaluate",
        params: { expression: "window.__clicks", returnByValue: true },
        full_access: true,
      }),
    ),
    "click counter evaluation",
  );
  if (evaluation.result?.Ok?.result?.result?.value !== 1) {
    throw new Error(`injected click did not reach the page: ${JSON.stringify(evaluation.result)}`);
  }

  // was_resized: new physical bounds must produce frames at the new size.
  await expectAck(
    send({
      kind: "set_bounds",
      tab_id: "tab-smoke",
      bounds: { x: 0, y: 0, width: 800, height: 600, scaleFactor: 1 },
    }),
    "set_bounds",
  );
  await waitFor(frameReady("tab-smoke", 800, 600), "resized frame_ready");

  await expectAck(
    send({ kind: "set_visible", tab_id: "tab-smoke", visible: false }),
    "set_visible",
  );
  await expectAck(send({ kind: "set_visible", tab_id: "tab-smoke", visible: true }), "re-show");

  // Navigation errors surface as structured tab_state_changed events.
  const navigationError = waitFor(
    (message) =>
      message.kind === "event" &&
      message.event?.kind === "tab_state_changed" &&
      message.event?.state?.navigationError?.failedUrl?.includes("hachimi-smoke.invalid"),
    "navigation error event",
  );
  await expectAck(
    send({ kind: "navigate", tab_id: "tab-smoke", url: "https://hachimi-smoke.invalid/" }),
    "navigate",
  );
  await navigationError;

  send({ kind: "shutdown" });
  child.stdin.end();
  const exitCode = await new Promise((resolveExit) => {
    const timer = setTimeout(() => resolveExit(null), 15000);
    child.on("exit", (code) => {
      clearTimeout(timer);
      resolveExit(code);
    });
  });
  if (exitCode !== 0) {
    throw new Error(`CEF host did not shut down cleanly (exit code ${exitCode})`);
  }

  console.log("CEF-HOST-SMOKE-OK");
} catch (error) {
  fail(error.message);
} finally {
  child.kill();
  server.close();
  rmSync(profile, { recursive: true, force: true });
}

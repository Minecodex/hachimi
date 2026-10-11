import { expect, test } from "@playwright/test";
import { setTimeout } from "node:timers";
import { clickWhenReady } from "../../../../scripts/desktop-e2e/support/interactions.mjs";

async function installDriver(page) {
  globalThis.browser = {
    execute: (callback, ...args) =>
      page.evaluate(
        ({ source, args }) => {
          const callable = (0, eval)(`(${source})`);
          return callable(...args);
        },
        { source: callback.toString(), args },
      ),
    waitUntil: async (condition, options) => {
      const end = Date.now() + options.timeout;
      while (Date.now() < end) {
        const value = await condition();
        if (value) return value;
        await new Promise((resolve) => setTimeout(resolve, 25));
      }
      throw new Error(options.timeoutMsg);
    },
    action: () => {
      let point;
      return {
        move(value) {
          point = value;
          return this;
        },
        perform: () => page.mouse.move(point.x, point.y),
      };
    },
  };
  globalThis.$ = (selector) => ({ click: () => page.locator(selector).click() });
}

test("native interaction helper reveals a hover-only control before hit testing", async ({
  page,
}) => {
  await page.goto("about:blank");
  await page.setContent(
    `<style>.row{padding:30px;width:200px}.row button{opacity:0;pointer-events:none}.row:hover button{opacity:1;pointer-events:auto}</style><div class="row"><button id="action" onclick="this.dataset.clicked='true'">Action</button></div>`,
  );
  await installDriver(page);
  try {
    await clickWhenReady("#action", 5000);
    await expect(page.locator("#action")).toHaveAttribute("data-clicked", "true");
  } finally {
    delete globalThis.browser;
    delete globalThis.$;
  }
});

test("native interaction helper clicks the intended target in a smooth scroll container", async ({
  page,
}) => {
  await page.goto("about:blank");
  await page.setContent(
    `<style>.scroll{height:200px;overflow:auto;scroll-behavior:smooth}.space{height:1200px}</style><div class="scroll"><div class="space"></div><button id="target" onclick="this.dataset.clicked='true'">Target</button><button id="other">Other</button></div>`,
  );
  await installDriver(page);
  try {
    await clickWhenReady("#target", 5000);
    await expect(page.locator("#target")).toHaveAttribute("data-clicked", "true");
    await expect(page.locator("#other")).not.toHaveAttribute("data-clicked", "true");
  } finally {
    delete globalThis.browser;
    delete globalThis.$;
  }
});

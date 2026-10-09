/* global HTMLButtonElement, HTMLElement, HTMLInputElement, XPathResult, document, getComputedStyle */

async function readyPoint(selector, timeout, requireEnabled, description) {
  return browser.waitUntil(
    async () =>
      browser.execute(
        (targetSelector, mustBeEnabled) => {
          const target = targetSelector.startsWith("/")
            ? document.evaluate(
                targetSelector,
                document,
                null,
                XPathResult.FIRST_ORDERED_NODE_TYPE,
                null,
              ).singleNodeValue
            : document.querySelector(targetSelector);
          if (!(target instanceof HTMLElement)) return false;
          target.scrollIntoView({ block: "center", inline: "nearest", behavior: "instant" });
          const style = getComputedStyle(target);
          const bounds = target.getBoundingClientRect();
          const disabled =
            mustBeEnabled &&
            (target instanceof HTMLButtonElement || target instanceof HTMLInputElement) &&
            target.disabled;
          if (
            disabled ||
            (mustBeEnabled && target.getAttribute("aria-disabled") === "true") ||
            style.display === "none" ||
            style.visibility === "hidden" ||
            bounds.width <= 0 ||
            bounds.height <= 0
          ) {
            return false;
          }
          const x = Math.round(bounds.left + bounds.width / 2);
          const y = Math.round(bounds.top + bounds.height / 2);
          const hit = document.elementFromPoint(x, y);
          if (mustBeEnabled && hit !== target && !target.contains(hit)) return false;
          return { x, y };
        },
        selector,
        requireEnabled,
      ),
    { timeout, timeoutMsg: `Element did not become ${description}: ${selector}` },
  );
}

export async function clickWhenReady(selector, timeout = 20_000) {
  const hoverPoint = await readyPoint(selector, timeout, true, "clickable");
  await browser.action("pointer").move({ duration: 0, x: hoverPoint.x, y: hoverPoint.y }).perform();
  const clickPoint = await readyPoint(selector, timeout, true, "clickable after pointer move");
  await browser
    .action("pointer")
    .move({ duration: 0, x: clickPoint.x, y: clickPoint.y })
    .down({ button: 0 })
    .up({ button: 0 })
    .perform();
}

export async function hoverWhenReady(selector, timeout = 20_000) {
  const point = await readyPoint(selector, timeout, false, "hoverable");
  await browser.action("pointer").move({ duration: 0, x: point.x, y: point.y }).perform();
}

export async function isDisplayed(selector) {
  return Boolean(
    await browser.execute((targetSelector) => {
      const target = targetSelector.startsWith("/")
        ? document.evaluate(
            targetSelector,
            document,
            null,
            XPathResult.FIRST_ORDERED_NODE_TYPE,
            null,
          ).singleNodeValue
        : document.querySelector(targetSelector);
      if (!(target instanceof HTMLElement)) return false;
      const style = getComputedStyle(target);
      const bounds = target.getBoundingClientRect();
      return (
        style.display !== "none" &&
        style.visibility !== "hidden" &&
        bounds.width > 0 &&
        bounds.height > 0
      );
    }, selector),
  );
}

export async function waitForDisplayed(selector, timeout = 20_000) {
  await browser.waitUntil(async () => isDisplayed(selector), {
    timeout,
    timeoutMsg: `Element did not become visible: ${selector}`,
  });
}

import { expect, test } from "@playwright/test";
import {
  beginAmbientObservation,
  readAmbientObservation,
  endAmbientObservation,
} from "../../../../scripts/desktop-e2e/support/motion-observation.mjs";

/* global document, requestAnimationFrame */

test("records a rendered ambient action that ends between driver polls", async ({ page }) => {
  await page.goto("about:blank");
  await page.setContent('<canvas class="pet-avatar-canvas" data-motion-slots="base"></canvas>');
  await page.evaluate(beginAmbientObservation);
  await page.evaluate(async () => {
    const canvas = document.querySelector(".pet-avatar-canvas");
    canvas.setAttribute("data-motion-ambient", "fixture.ambient.once");
    canvas.setAttribute("data-motion-slots", "base,action");
    await new Promise((resolve) => requestAnimationFrame(resolve));
    canvas.setAttribute("data-motion-slots", "base");
  });
  await expect(page.locator("canvas")).toHaveAttribute("data-motion-slots", "base");
  expect(await page.evaluate(readAmbientObservation)).toBe("fixture.ambient.once");
  await page.evaluate(endAmbientObservation);
  expect(await page.evaluate(readAmbientObservation)).toBe(false);
});

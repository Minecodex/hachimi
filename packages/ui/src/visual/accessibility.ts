import { expect } from "@playwright/test";

// Storybook's accessibility addon and an explicit Playwright audit share Axe.
// Wait only for that known engine lock; violations and other failures remain fatal.
export async function runAxeWhenAvailable<T>(scan: () => Promise<T>): Promise<T> {
  let result: { value: T } | undefined;
  let failure: { error: unknown } | undefined;
  await expect
    .poll(
      async () => {
        try {
          result = { value: await scan() };
        } catch (error) {
          if (error instanceof Error && error.message.includes("Axe is already running")) {
            return false;
          }
          failure = { error };
        }
        return true;
      },
      { timeout: 10_000, intervals: [100, 250, 500] },
    )
    .toBe(true);
  if (failure) throw failure.error;
  if (!result) throw new Error("Accessibility audit did not produce a result");
  return result.value;
}

import { afterEach, describe, expect, it, vi } from "vitest";
import { withMotionFeatureCacheBudget } from "./motion-feature-cache-budget";

afterEach(() => vi.useRealTimers());

describe("optional motion feature cache", () => {
  it("preserves successful cache payloads and clears the timer", async () => {
    vi.useFakeTimers();
    expect(await withMotionFeatureCacheBudget(Promise.resolve("cached"))).toBe("cached");
    expect(vi.getTimerCount()).toBe(0);
  });

  it("releases a hung native cache request so immutable source analysis can continue", async () => {
    vi.useFakeTimers();
    const request = withMotionFeatureCacheBudget(new Promise<never>(() => {}));
    const assertion = expect(request).rejects.toThrow("Motion feature cache did not respond");
    await vi.advanceTimersByTimeAsync(2_000);
    await assertion;
    expect(vi.getTimerCount()).toBe(0);
  });
});

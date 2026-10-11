import { describe, expect, it, vi } from "vitest";
import { runAxeWhenAvailable } from "./visual/accessibility";

describe("explicit accessibility audit", () => {
  it("waits for the addon lock and preserves the complete audit result", async () => {
    const result = { violations: [{ id: "color-contrast" }] };
    const scan = vi
      .fn()
      .mockRejectedValueOnce(new Error("Axe is already running. Use await axe.run()"))
      .mockResolvedValue(result);
    expect(await runAxeWhenAvailable(scan)).toBe(result);
    expect(scan).toHaveBeenCalledTimes(2);
  });

  it("propagates other audit errors without retrying them", async () => {
    const failure = new Error("Audit script failed");
    const scan = vi.fn().mockRejectedValue(failure);
    await expect(runAxeWhenAvailable(scan)).rejects.toBe(failure);
    expect(scan).toHaveBeenCalledTimes(1);
  });
});

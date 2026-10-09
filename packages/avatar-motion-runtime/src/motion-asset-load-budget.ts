const MOTION_ASSET_STAGE_BUDGET_MS = 20_000;

export async function withMotionAssetLoadBudget<T>(
  operation: Promise<T>,
  stage: string,
): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      operation,
      new Promise<never>((_resolve, reject) => {
        timer = setTimeout(
          () => reject(new Error(`Motion asset ${stage} did not respond`)),
          MOTION_ASSET_STAGE_BUDGET_MS,
        );
      }),
    ]);
  } finally {
    if (timer !== undefined) clearTimeout(timer);
  }
}

const FEATURE_CACHE_BUDGET_MS = 2_000;

// Cache IPC is optional. A lost native response must not prevent source analysis.
export async function withMotionFeatureCacheBudget<T>(operation: Promise<T>): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      operation,
      new Promise<never>((_resolve, reject) => {
        timer = setTimeout(
          () => reject(new Error("Motion feature cache did not respond")),
          FEATURE_CACHE_BUDGET_MS,
        );
      }),
    ]);
  } finally {
    if (timer !== undefined) clearTimeout(timer);
  }
}

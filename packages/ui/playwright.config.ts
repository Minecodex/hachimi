import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "./src/visual",
  ...(process.env.CI ? { workers: 1 } : {}),
  snapshotPathTemplate: "{testDir}/__screenshots__/{arg}{ext}",
  use: {
    locale: "en-US",
    timezoneId: "Asia/Shanghai",
    baseURL: "http://127.0.0.1:6007",
    viewport: { width: 1280, height: 720 },
  },
  webServer: [
    {
      command: "corepack pnpm exec http-server storybook-static -a 127.0.0.1 -p 6007 -c-1",
      port: 6007,
      reuseExistingServer: true,
    },
    {
      command: "corepack pnpm --filter @hachimi/desktop-web dev",
      port: 1420,
      reuseExistingServer: true,
    },
  ],
});

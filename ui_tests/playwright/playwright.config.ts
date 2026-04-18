import { defineConfig, devices } from "@playwright/test";

// Omnibus UI e2e tests.
//
// Requires a locally running server at http://127.0.0.1:3000 (e.g. `cargo run
// -p omnibus` or `dx serve --port 3000 --package omnibus`). There's
// intentionally no `webServer` block — the server lifecycle is managed by the
// developer so tests stay predictable against hot-reload workflows.
export default defineConfig({
  testDir: "./tests",
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 2 : 0,
  reporter: "list",
  use: {
    baseURL: "http://127.0.0.1:3000",
    trace: "on-first-retry",
    screenshot: "only-on-failure",
  },
  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
    },
  ],
});

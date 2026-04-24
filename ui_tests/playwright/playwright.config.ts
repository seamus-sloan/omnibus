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
  globalSetup: require.resolve("./globalSetup.ts"),
  use: {
    baseURL: "http://127.0.0.1:3000",
    trace: "on-first-retry",
    screenshot: "only-on-failure",
    storageState: "./.auth/storage.json",
    // Server-side `origin_check` middleware rejects cookie-authed
    // state-changing requests with no `Origin`/`Referer`. Playwright's
    // APIRequestContext (used by `globalSetup` and seeding helpers) does
    // not send Origin by default, so attach one matching `baseURL` to
    // every request the suite makes — both browser and API.
    extraHTTPHeaders: {
      Origin: "http://127.0.0.1:3000",
    },
  },
  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
    },
  ],
});

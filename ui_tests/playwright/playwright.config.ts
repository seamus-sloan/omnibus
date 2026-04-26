import { defineConfig, devices } from "@playwright/test";

import { STORAGE_STATE_PATH } from "./globalSetup";

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
    // Absolute path so the writer (`globalSetup`) and reader (test contexts)
    // agree regardless of CWD — `npx playwright test -c …` from the repo
    // root and `cd ui_tests/playwright && npx playwright test` both hit the
    // same file.
    storageState: STORAGE_STATE_PATH,
    // Server-side `origin_check` middleware rejects cookie-authed
    // state-changing requests with no `Origin`/`Referer`. Playwright does
    // not send Origin by default, so set it here for browser pages and the
    // spec-level `request` fixture. `globalSetup` reads this same value
    // from `config.projects[0].use` and passes it into its own
    // APIRequestContext so register/login (and any future state-changing
    // setup call) inherit the Origin too.
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

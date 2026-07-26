import { defineConfig, devices } from "@playwright/test";

import { STORAGE_STATE_PATH } from "./globalSetup";

// Omnibus UI e2e tests.
//
// Requires a locally running server. The base URL comes from
// $PLAYWRIGHT_BASE_URL (set by `scripts/dev-server-up.sh` in
// `.claude/runtime/env.sh`); falls back to http://127.0.0.1:3000 for
// humans running `cargo run -p omnibus` directly. There's intentionally
// no `webServer` block — the server lifecycle is managed externally so
// tests stay predictable against hot-reload workflows and shared by
// multiple parallel agents.
const BASE_URL = process.env.PLAYWRIGHT_BASE_URL ?? "http://127.0.0.1:3000";

export default defineConfig({
  testDir: "./tests",
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 2 : 0,
  // `list` for readable console output; `junit` for Codecov Test Analytics.
  // CI overrides this via `--reporter=…` (it also adds `blob` for the merged
  // HTML report and sets PLAYWRIGHT_JUNIT_OUTPUT_NAME per shard).
  reporter: [["list"], ["junit", { outputFile: "results.xml" }]],
  // Most specs seed in a `beforeAll` that polls the indexer for up to 45s
  // (see `pollForBookCount` — cold GitHub-runner indexing of the full
  // public-domain set measured 27–36s). Hooks inherit the test timeout, so
  // the Playwright 30s default cut the seed off before the poll could finish.
  // 60s clears a single-library seed with margin; dual-library seeding hooks
  // (two sequential polls) raise their own timeout via `test.setTimeout`.
  timeout: 60_000,
  globalSetup: require.resolve("./globalSetup.ts"),
  use: {
    baseURL: BASE_URL,
    trace: "on-first-retry",
    screenshot: "only-on-failure",
    // Absolute path so the writer (`globalSetup`) and reader (test contexts)
    // agree regardless of CWD — `pnpm exec playwright test -c …` from the
    // repo root and `cd ui_tests/playwright && pnpm exec playwright test`
    // both hit the same file.
    storageState: STORAGE_STATE_PATH,
    // Server-side `origin_check` middleware rejects cookie-authed
    // state-changing requests with no `Origin`/`Referer`. Playwright does
    // not send Origin by default, so set it here for browser pages and the
    // spec-level `request` fixture. `globalSetup` reads this same value
    // from `config.projects[0].use` and passes it into its own
    // APIRequestContext so register/login (and any future state-changing
    // setup call) inherit the Origin too. Tracks baseURL so an alternate
    // port (set via PLAYWRIGHT_BASE_URL) stays origin-allowed.
    extraHTTPHeaders: {
      Origin: BASE_URL,
    },
  },
  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
    },
  ],
});

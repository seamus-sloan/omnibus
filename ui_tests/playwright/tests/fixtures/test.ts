// Extended `test` / `expect` for the omnibus Playwright suite.
//
// All flow specs import from this file (not `@playwright/test` directly) so
// shared fixtures and project-level conventions stay in one place. Today the
// only shared piece of state is `seededLibrary` — a worker-scoped fixture
// that seeds the running server against the committed EPUB fixtures.
//
// Spec opt-in: any flow that asserts against fixture content adds
// `seededLibrary` to a `test.beforeAll` parameter list so the fixture runs
// before the first test in that worker. Flows that don't care about the
// library (auth / settings / theme) leave it out and avoid the seed cost.
import { request as apiRequest, test as base, expect } from "@playwright/test";

import { FIXTURE_BOOKS } from "./epubs";
import { fixturesDir, seedLibrary } from "../utils/seed";

type WorkerFixtures = {
  // Marker fixture — depend on it in a `test.beforeAll(({ seededLibrary }) => {})`
  // to ensure the library has been seeded before any test in the file runs.
  seededLibrary: void;
};

export const test = base.extend<{}, WorkerFixtures>({
  seededLibrary: [
    // Worker-scoped fixtures cannot use the test-scoped `request` fixture, so
    // build a one-off APIRequestContext that mirrors the project `use` block
    // (baseURL, Origin header, persisted storage state from globalSetup).
    // Storage state matters: `seedLibrary` POSTs `/api/rpc/settings` which
    // requires an authenticated admin session.
    async ({ playwright }, use, workerInfo) => {
      const projectUse = workerInfo.project.use;
      const baseURL = projectUse.baseURL;
      const extraHTTPHeaders = projectUse.extraHTTPHeaders;
      const storageState =
        typeof projectUse.storageState === "string" ? projectUse.storageState : undefined;
      const ctx = await apiRequest.newContext({ baseURL, extraHTTPHeaders, storageState });
      try {
        await seedLibrary(ctx, fixturesDir(), FIXTURE_BOOKS.length);
        await use();
      } finally {
        await ctx.dispose();
      }
    },
    { scope: "worker" },
  ],
});

export { expect };

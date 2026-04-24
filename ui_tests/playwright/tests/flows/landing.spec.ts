import { expect, test } from "../fixtures/test";
import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { expectNavVisible, gotoReady } from "../utils/nav";
import { expectRowMatches } from "../utils/ebooks";
import { fixturesDir, seedLibrary } from "../utils/seed";

// Seed the running server against the committed EPUB fixtures before any
// landing-page assertion runs. The settings POST kicks off an async reindex
// inside the server (`tokio::spawn`), so `seedLibrary` polls
// `/api/rpc/ebooks` until the indexer has surfaced every fixture.
test.beforeAll(async ({ request }) => {
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
});

test("renders the landing page layout", async ({ page }) => {
  await gotoReady(page, "/");

  await expect(page.getByRole("heading", { level: 1, name: "Your Library" })).toBeVisible();
  await expect(page.getByTestId("ebook-table")).toBeVisible();
  await expectNavVisible(page);
});

test("renders every fixture book with the expected metadata", async ({ page }) => {
  await gotoReady(page, "/");

  // Lock in the count so a stray test EPUB on disk fails loudly instead of
  // silently passing the per-row assertions for the books we know about.
  await expect(page.getByTestId(/^ebook-row-/)).toHaveCount(FIXTURE_BOOKS.length);

  for (const expected of FIXTURE_BOOKS) {
    await expectRowMatches(page, expected);
  }
});

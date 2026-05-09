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

// View prefs persist in localStorage; clear before each test so one spec's
// state can't leak into the next one's table-vs-grid assumption.
test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    try {
      window.localStorage.clear();
    } catch {
      /* private mode — nothing to clear */
    }
  });
});

test("renders the landing page layout", async ({ page }) => {
  await gotoReady(page, "/");

  await expect(page.getByRole("heading", { level: 1, name: "Your Library" })).toBeVisible();
  await expect(page.getByTestId("ebook-table")).toBeVisible();
  await expect(page.getByTestId("lib-toolbar")).toBeVisible();
  await expect(page.getByTestId("lib-sidebar")).toBeVisible();
  await expectNavVisible(page);
});

test("renders every fixture book with the expected metadata", async ({ page }) => {
  await gotoReady(page, "/");

  // Lock in the count so a stray test EPUB on disk fails loudly instead of
  // silently passing the per-row assertions for the books we know about.
  await expect(page.getByTestId(/^ebook-row-/)).toHaveCount(FIXTURE_BOOKS.length);

  for (const expected of FIXTURE_BOOKS) {
    await test.step(`renders "${expected.title}" from ${expected.filename}`, async () => {
      await expectRowMatches(page, expected);
    });
  }
});

test("toggles to grid view and persists across reload", async ({ page }) => {
  await gotoReady(page, "/");

  await expect(page.getByTestId("ebook-table")).toBeVisible();
  await page.getByTestId("view-toggle-grid").click();

  await expect(page.getByTestId("lib-grid")).toBeVisible();
  await expect(page.getByTestId("ebook-table")).toHaveCount(0);
  await expect(page.getByTestId(/^ebook-tile-/)).toHaveCount(FIXTURE_BOOKS.length);
  await expect(page.getByTestId("view-toggle-grid")).toHaveAttribute("aria-pressed", "true");

  await page.reload();
  await expect(page.getByTestId("lib-grid")).toBeVisible();
  await expect(page.getByTestId("ebook-table")).toHaveCount(0);
});

test("sorts by title descending when the Title header is clicked", async ({ page }) => {
  await gotoReady(page, "/");

  // Default sort is title asc — click once to flip to desc.
  await page.getByRole("button", { name: /^Title( ▲| ▼)?$/ }).click();

  const titleHeader = page.locator(".sort-th[aria-sort='descending']");
  await expect(titleHeader).toBeVisible();

  // First row's title cell should match the alphabetically-last fixture.
  const titles = [...FIXTURE_BOOKS.map((b) => b.title)].sort((a, b) =>
    a.toLowerCase().localeCompare(b.toLowerCase()),
  );
  const lastTitle = titles[titles.length - 1];
  await expect(page.getByTestId(/^ebook-row-/).first().getByTestId("ebook-cell-title")).toHaveText(
    lastTitle,
  );
});

test("filters by author chip and clears via the clear-all button", async ({ page }) => {
  await gotoReady(page, "/");
  await expect(page.getByTestId(/^ebook-row-/)).toHaveCount(FIXTURE_BOOKS.length);

  const authorsFacet = page.getByTestId("lib-facet-authors");
  const lovelaceChip = authorsFacet.locator('button.lib-chip[data-value="Ada Lovelace"]');
  await lovelaceChip.click();

  await expect(lovelaceChip).toHaveAttribute("aria-pressed", "true");
  // Only one fixture is by Ada Lovelace.
  await expect(page.getByTestId(/^ebook-row-/)).toHaveCount(1);
  await expect(
    page.getByTestId(/^ebook-row-/).first().getByTestId("ebook-cell-author"),
  ).toHaveText("Ada Lovelace");

  await page.getByTestId("lib-clear-filters").click();
  await expect(lovelaceChip).toHaveAttribute("aria-pressed", "false");
  await expect(page.getByTestId(/^ebook-row-/)).toHaveCount(FIXTURE_BOOKS.length);
});

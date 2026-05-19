import { expect, test } from "../fixtures/test";
import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { expectNavVisible, gotoReady } from "../utils/nav";
import { expectRowMatches } from "../utils/ebooks";

// Depend on the worker-scoped `seededLibrary` fixture so the server is
// indexed against the committed EPUB fixtures before any test in this file
// runs. The fixture is opt-in (not `auto: true`) so flows that don't care
// about library state — auth, settings, theme — skip the seed cost.
test.beforeAll(({ seededLibrary }) => {
  void seededLibrary;
});

// View prefs persist in localStorage. Each Playwright test gets a fresh
// browser context and storageState's `origins` is empty (only the auth
// cookie is saved), so localStorage starts empty per test without needing
// an explicit cleanup hook. A `page.addInitScript` clear() would run on
// `page.reload()` too — wiping the very state the persistence test asserts.

test("renders the landing page layout", async ({ page }) => {
  await gotoReady(page, "/");

  await expect(page.getByRole("heading", { level: 1, name: "Your Library" })).toBeVisible();
  await expect(page.getByTestId("ebook-table")).toBeVisible();
  await expect(page.getByTestId("lib-toolbar")).toBeVisible();
  // Sidebar is collapsed by default; opening it via the toolbar toggle
  // exercises the new persisted preference and confirms the sidebar
  // markup is wired up.
  const filtersToggle = page.getByTestId("lib-filters-toggle");
  await expect(filtersToggle).toHaveAttribute("aria-pressed", "false");
  await filtersToggle.click();
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
  await page.getByRole("button", { name: /^Title( ↑| ↓)?$/ }).click();

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

  // Sidebar starts collapsed; open it before reaching for the chip.
  await page.getByTestId("lib-filters-toggle").click();

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

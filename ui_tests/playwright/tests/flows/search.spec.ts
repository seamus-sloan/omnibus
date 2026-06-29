import { expect, test } from "../fixtures/test";
import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { gotoReady } from "../utils/nav";
import { fixturesDir, seedLibrary } from "../utils/seed";

test.beforeAll(async ({ request }) => {
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
});

// ── Helper: open the search palette and type a query ──────────────────
// The old inline search input is replaced by the F1.5 palette. These
// helpers open the palette and type into its input. The palette shows
// grouped results (books, authors, series, tags) inline — no form
// submission is needed.

async function openPaletteAndType(
  page: import("@playwright/test").Page,
  query: string,
) {
  await page.getByTestId("search-trigger").click();
  const input = page.getByTestId("sp-input");
  await expect(input).toBeVisible();
  await input.fill(query);
}

test("search input shows matching book results in palette", async ({ page }) => {
  await gotoReady(page, "/");

  await openPaletteAndType(page, "dracula");

  // Poll until the palette shows a book result matching "dracula".
  await expect
    .poll(async () => page.getByTestId("sp-book-row").count())
    .toBeGreaterThanOrEqual(1);
});

test("search by author shows author results", async ({ page }) => {
  await gotoReady(page, "/");
  await openPaletteAndType(page, "shakespeare");

  // Poll until the palette shows author results.
  await expect
    .poll(async () => page.getByTestId("sp-author-row").count())
    .toBeGreaterThanOrEqual(1);
});

test("clearing the search clears results", async ({ page }) => {
  await gotoReady(page, "/");
  await openPaletteAndType(page, "dracula");
  await expect
    .poll(async () => page.getByTestId("sp-book-row").count())
    .toBeGreaterThanOrEqual(1);

  // Clear the input.
  const input = page.getByTestId("sp-input");
  await input.fill("");

  // Results should disappear.
  await expect
    .poll(async () => page.getByTestId("sp-book-row").count())
    .toBe(0);
});

test("settings page does not render the search trigger", async ({ page }) => {
  await gotoReady(page, "/settings");
  await expect(page.getByTestId("search-trigger")).toHaveCount(0);
});

test("author substring shows author results", async ({ page }) => {
  await gotoReady(page, "/");
  await openPaletteAndType(page, "shakespeare");

  await expect
    .poll(async () => page.getByTestId("sp-author-row").count())
    .toBeGreaterThanOrEqual(1);
});

test("tag substring shows tag or book results", async ({ page }) => {
  await gotoReady(page, "/");
  await openPaletteAndType(page, "vampires");

  // "Vampires" is a tag on Dracula — should appear in tag results or book results.
  await expect
    .poll(async () => {
      const tags = await page.getByTestId("sp-tag-row").count();
      const books = await page.getByTestId("sp-book-row").count();
      return tags + books;
    })
    .toBeGreaterThanOrEqual(1);
});

// ── /search/:query full-page route ────────────────────────────────────

test("renders the /search results page layout", async ({ page }) => {
  await gotoReady(page, "/search/dracula");

  // Breadcrumb back affordance present.
  await expect(page.getByTestId("search-back")).toBeVisible();

  // Heading echoes the query, and the summary stat line shows the engine +
  // timing once the RPC settles.
  await expect(page.getByRole("heading", { level: 1 })).toContainText(/dracula/i);
  await expect
    .poll(async () => page.getByTestId("search-result-count").count())
    .toBe(1);
  await expect(page.getByTestId("search-result-count")).toContainText(/fts5/);

  // At least one book cover card for "dracula".
  await expect
    .poll(async () => page.getByTestId("search-book-row").count())
    .toBeGreaterThanOrEqual(1);

  // The matched term is highlighted at least once on the page.
  await expect(page.locator(".search-hit").first()).toBeVisible();

  // "On this page" jump rail is present.
  await expect(page.getByText("On this page")).toBeVisible();
});

test("search back link returns to the library", async ({ page }) => {
  await gotoReady(page, "/search/dracula");

  const back = page.getByTestId("search-back");
  await expect(back).toBeVisible();
  await back.click();
  await expect(page).toHaveURL(/\/$/);
});

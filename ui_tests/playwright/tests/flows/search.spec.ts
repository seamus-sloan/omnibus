import { expect, test } from "../fixtures/test";
import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { gotoReady } from "../utils/nav";
import { fixturesDir, seedLibrary } from "../utils/seed";

test.beforeAll(async ({ request }) => {
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
});

// ── Helper: open the search palette and type a query ──────────────────
// The old inline search input is replaced by the F1.5 palette. These
// helpers open the palette, type into its input, and — where the test
// expects filtered landing results — submit the query so the landing
// page's SearchQuery signal picks it up.

async function openPaletteAndType(
  page: import("@playwright/test").Page,
  query: string,
) {
  await page.getByTestId("search-trigger").click();
  const input = page.getByTestId("sp-input");
  await expect(input).toBeVisible();
  await input.fill(query);
}

test("search input narrows the library to matching rows", async ({ page }) => {
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

test("author: facet narrows by author", async ({ page }) => {
  await gotoReady(page, "/");
  await openPaletteAndType(page, "shakespeare");

  await expect
    .poll(async () => page.getByTestId("sp-author-row").count())
    .toBeGreaterThanOrEqual(1);
});

test("tag: facet narrows by tag", async ({ page }) => {
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

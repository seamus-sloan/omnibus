import { expect, test } from "../fixtures/test";
import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { gotoReady } from "../utils/nav";
import { fixturesDir, seedLibrary } from "../utils/seed";

test.beforeAll(async ({ request }) => {
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
});

test("search input narrows the library to matching rows", async ({ page }) => {
  await gotoReady(page, "/");

  const search = page.getByRole("searchbox", { name: "Search books" });
  await expect(search).toBeVisible();

  await search.fill("dracula");

  // Poll until the table updates — the search kicks off an async fetch that
  // races the input event. `expect.poll` respects Playwright's auto-retry.
  await expect.poll(async () => page.getByTestId(/^ebook-row-/).count()).toBe(1);

  await expect(page.getByTestId("ebook-row-dracula")).toBeVisible();
});

test("search by author pulls matching books across the library", async ({ page }) => {
  await gotoReady(page, "/");
  const search = page.getByRole("searchbox", { name: "Search books" });

  await search.fill("shakespeare");
  await expect.poll(async () => page.getByTestId(/^ebook-row-/).count()).toBe(1);
  await expect(page.getByTestId("ebook-row-romeo-and-juliet")).toBeVisible();
});

test("clearing the search restores the full library", async ({ page }) => {
  await gotoReady(page, "/");
  const search = page.getByRole("searchbox", { name: "Search books" });

  await search.fill("dracula");
  await expect
    .poll(async () => page.getByTestId(/^ebook-row-/).count())
    .toBe(1);

  await search.fill("");
  await expect
    .poll(async () => page.getByTestId(/^ebook-row-/).count())
    .toBe(FIXTURE_BOOKS.length);
});

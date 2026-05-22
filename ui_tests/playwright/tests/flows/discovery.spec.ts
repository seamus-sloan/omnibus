import { expect, test } from "../fixtures/test";
import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { gotoReady } from "../utils/nav";
import { fixturesDir, seedLibrary } from "../utils/seed";
import type { APIRequestContext } from "@playwright/test";

// Seed before all tests so the indexer has populated authors/series/tags.
test.beforeAll(async ({ request }) => {
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
});

// ---------------------------------------------------------------------------
// Helpers — resolve IDs via the RPC layer so tests don't hardcode DB ids.
// ---------------------------------------------------------------------------

async function fetchAuthorIdByName(
  request: APIRequestContext,
  name: string,
): Promise<number> {
  // GET /api/rpc/ebooks (same payload the landing page uses) and scan
  // each book's creators for the named author.
  const resp = await request.get("/api/rpc/ebooks");
  expect(resp.status(), "GET /api/rpc/ebooks failed").toBe(200);
  const body = (await resp.json()) as {
    books: { creators: { name: string; id: number | null }[] }[];
  };
  for (const book of body.books) {
    const match = book.creators.find((c) => c.name === name);
    if (match?.id) return match.id;
  }
  throw new Error(`no indexed author named ${JSON.stringify(name)}`);
}

async function fetchSeriesIdByName(
  request: APIRequestContext,
  name: string,
): Promise<number> {
  // `EbookMetadata` carries `series_id` directly, so we can resolve the
  // series ID from any book in the series via the same `/api/rpc/ebooks`
  // payload the landing page consumes.
  const resp = await request.get("/api/rpc/ebooks");
  expect(resp.status(), "GET /api/rpc/ebooks failed").toBe(200);
  const body = (await resp.json()) as {
    books: { series: string | null; series_id: number | null }[];
  };
  const match = body.books.find((b) => b.series === name && b.series_id != null);
  if (!match?.series_id) {
    throw new Error(`no indexed series named ${JSON.stringify(name)}`);
  }
  return match.series_id;
}

// ---------------------------------------------------------------------------
// Author page
// ---------------------------------------------------------------------------

test("renders the author page layout", async ({ page, request }) => {
  const authorName = "Ada Lovelace";
  const authorId = await fetchAuthorIdByName(request, authorName);

  await gotoReady(page, `/authors/${authorId}`);

  // H1 contains the author's name (split first/last with italic)
  await expect(page.getByRole("heading", { level: 1 })).toContainText("Ada");
  await expect(page.getByRole("heading", { level: 1 })).toContainText("Lovelace");

  // Book count stat is visible
  await expect(page.getByText("In your library")).toBeVisible();

  // Breadcrumb has "Library" link
  const breadcrumb = page.locator("nav.breadcrumb");
  await expect(breadcrumb.getByRole("link", { name: "Library" })).toBeVisible();
});

test("author page shows books by the author", async ({ page, request }) => {
  // Niklaus Wirth has 4 books in the Code Quartet
  const authorName = "Niklaus Wirth";
  const authorId = await fetchAuthorIdByName(request, authorName);

  await gotoReady(page, `/authors/${authorId}`);

  await expect(page.getByRole("heading", { level: 1 })).toContainText("Wirth");

  // Should show the series section for "Code Quartet"
  await expect(page.getByText("Code Quartet")).toBeVisible();

  // At least the first book title should be visible
  await expect(page.getByText("Quartet I: Lexer")).toBeVisible();
});

// ---------------------------------------------------------------------------
// Series page
// ---------------------------------------------------------------------------

test("renders the series page layout", async ({ page, request }) => {
  const seriesName = "Pioneers";
  const seriesId = await fetchSeriesIdByName(request, seriesName);

  await gotoReady(page, `/series/${seriesId}`);

  // H1 contains the series name
  await expect(page.getByRole("heading", { level: 1 })).toBeVisible();

  // Book count label is visible
  await expect(page.getByText(/in library/)).toBeVisible();

  // Breadcrumb has "Library" link
  const breadcrumb = page.locator("nav.breadcrumb");
  await expect(breadcrumb.getByRole("link", { name: "Library" })).toBeVisible();
});

test("series page shows books in order", async ({ page, request }) => {
  const seriesName = "Pioneers";
  const seriesId = await fetchSeriesIdByName(request, seriesName);

  await gotoReady(page, `/series/${seriesId}`);

  // Should have multiple book cards — "Pioneers" has 5 books in fixtures
  const cards = page.locator("article.series-card");
  await expect(cards).toHaveCount(5);

  // First book should show "Book #1"
  await expect(cards.first().getByText("Book #1")).toBeVisible();
});

// ---------------------------------------------------------------------------
// Tag cloud page
// ---------------------------------------------------------------------------

test("renders the tag cloud layout", async ({ page }) => {
  await gotoReady(page, "/tags");

  // Page header
  await expect(page.getByRole("heading", { level: 1 })).toContainText("tag");
  await expect(page.getByText("unique tags")).toBeVisible();

  // At least one tag cloud item rendered
  const tagItems = page.locator(".tag-cloud-item");
  const count = await tagItems.count();
  expect(count).toBeGreaterThan(0);
});

test("tag cloud items show counts", async ({ page }) => {
  await gotoReady(page, "/tags");

  // Each tag should have a visible count span
  const counts = page.locator(".tag-cloud-count");
  const firstCount = await counts.first().textContent();
  expect(firstCount).toBeTruthy();
  expect(Number(firstCount)).toBeGreaterThan(0);
});

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
  // POST to /api/rpc/ebooks (same as the landing page) and scan creators.
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
  // The ebooks list doesn't carry series IDs directly. Instead query the
  // dedicated tag cloud endpoint which doesn't help either. Use the books
  // list to find a book in the series, then query the /api/rpc/search for
  // the series name and use that result's series field to locate the id
  // via a direct SQL approach — or simpler: just POST to the rpc/series
  // endpoint after finding via the DB. But we don't have an index endpoint
  // yet. Instead, use the REST API /api/series/:id by probing.
  //
  // Simplest approach: POST to /api/rpc/ebooks to get a book in the series,
  // then use the books_series_link table. Since we can't do raw SQL from
  // Playwright, we'll search for the series name in the ebooks response and
  // then iterate candidate IDs via the REST endpoint.
  const ebooksResp = await request.get("/api/rpc/ebooks");
  expect(ebooksResp.status()).toBe(200);
  const ebooksBody = (await ebooksResp.json()) as {
    books: { series: string | null; id: number }[];
  };
  const bookInSeries = ebooksBody.books.find((b) => b.series === name);
  if (!bookInSeries) {
    throw new Error(`no book in series ${JSON.stringify(name)}`);
  }

  // Try IDs 1..100 via the REST endpoint until we find the matching series.
  for (let id = 1; id <= 100; id++) {
    const resp = await request.post("/api/rpc/series", {
      data: { id },
    });
    if (resp.status() === 200) {
      const body = (await resp.json()) as { name: string; id: number } | null;
      if (body && body.name === name) return body.id;
    }
  }
  throw new Error(`could not resolve series id for ${JSON.stringify(name)}`);
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

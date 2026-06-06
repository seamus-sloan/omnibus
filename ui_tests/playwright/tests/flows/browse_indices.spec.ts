import { expect, test } from "../fixtures/test";
import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { gotoReady } from "../utils/nav";
import { fixturesDir, seedLibrary } from "../utils/seed";

// F1.12 — Browse authors & series index pages.
//
// Seed the library once before all tests so the indexer has populated the
// `authors` and `series` tables that the index queries aggregate over.
test.beforeAll(async ({ request }) => {
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
});

// ---------------------------------------------------------------------------
// /authors index
// ---------------------------------------------------------------------------

test("renders the authors index layout", async ({ page }) => {
  await gotoReady(page, "/authors");

  // Hero — "By author."
  await expect(page.getByRole("heading", { level: 1 })).toContainText("author");

  // Breadcrumb has "Library" link.
  const breadcrumb = page.locator("nav.breadcrumb");
  await expect(breadcrumb.getByRole("link", { name: "Library" })).toBeVisible();

  // Toolbar — filter input and at least one sort button.
  await expect(page.getByTestId("authors-filter")).toBeVisible();
  await expect(page.getByTestId("authors-sort-name")).toBeVisible();
  await expect(page.getByTestId("authors-sort-count")).toBeVisible();

  // At least one author card was rendered.
  const cards = page.getByTestId("author-card");
  await expect(cards.first()).toBeVisible();
  const count = await cards.count();
  expect(count).toBeGreaterThan(0);

  // Nav present.
  const nav = page.getByRole("navigation").first();
  await expect(nav.getByRole("link", { name: "Authors" })).toBeVisible();
});

test("authors index filters by name", async ({ page }) => {
  await gotoReady(page, "/authors");

  const initialCount = await page.getByTestId("author-card").count();
  expect(initialCount).toBeGreaterThan(1);

  // Filter to a single author known to be in the fixture set.
  await page.getByTestId("authors-filter").fill("Lovelace");

  // Poll until only matching cards remain.
  await expect.poll(async () => page.getByTestId("author-card").count())
    .toBeLessThan(initialCount);

  const visible = await page.getByTestId("author-card").allTextContents();
  for (const text of visible) {
    expect(text.toLowerCase()).toContain("lovelace");
  }
});

test("authors index letter strip jumps to the matching section", async ({ page }) => {
  await gotoReady(page, "/authors");

  // Wait for the index to render so the letter strip has populated.
  // Ada Lovelace is in the fixture set, so the surname-sort bucket "L"
  // is guaranteed to be present.
  await expect(page.getByTestId("author-card").first()).toBeVisible();

  const letter = page.getByTestId("authors-letter-L");
  await expect(letter).toBeVisible();
  // Active letters render as same-page anchors so the browser scrolls
  // natively to the matching section — no JS required.
  await expect(letter).toHaveAttribute("href", "#letter-L");

  await letter.click();

  await expect(page).toHaveURL(/#letter-L$/);
  const section = page.locator("#letter-L");
  await expect(section).toBeInViewport();
});

test("authors index card navigates to the detail page", async ({ page }) => {
  await gotoReady(page, "/authors");

  // Filter so the card we click is deterministic.
  await page.getByTestId("authors-filter").fill("Lovelace");
  const card = page.getByTestId("author-card").first();
  await expect(card).toBeVisible();

  // Capture the destination id from the card's href so we can assert on
  // the URL without depending on a post-navigation server-function fetch
  // (which is HMR-sensitive against the live dev server). The author
  // detail page itself is covered by discovery.spec.ts.
  const href = await card.getAttribute("href");
  expect(href).toMatch(/^\/authors\/\d+$/);

  await card.click();
  await expect(page).toHaveURL(new RegExp(`${href}$`));
});

// ---------------------------------------------------------------------------
// /series index
// ---------------------------------------------------------------------------

test("renders the series index layout", async ({ page }) => {
  await gotoReady(page, "/series");

  await expect(page.getByRole("heading", { level: 1 })).toContainText("series");

  const breadcrumb = page.locator("nav.breadcrumb");
  await expect(breadcrumb.getByRole("link", { name: "Library" })).toBeVisible();

  await expect(page.getByTestId("series-filter")).toBeVisible();
  await expect(page.getByTestId("series-sort-name")).toBeVisible();
  await expect(page.getByTestId("series-sort-count")).toBeVisible();

  const cards = page.getByTestId("series-card");
  await expect(cards.first()).toBeVisible();
  expect(await cards.count()).toBeGreaterThan(0);
});

test("series index filters by name", async ({ page }) => {
  await gotoReady(page, "/series");

  const initialCount = await page.getByTestId("series-card").count();

  await page.getByTestId("series-filter").fill("Pioneers");
  await expect.poll(async () => page.getByTestId("series-card").count())
    .toBeLessThanOrEqual(initialCount);

  const visible = await page.getByTestId("series-card").allTextContents();
  for (const text of visible) {
    expect(text.toLowerCase()).toContain("pioneer");
  }
});

test("series index card navigates to the detail page", async ({ page }) => {
  await gotoReady(page, "/series");

  await page.getByTestId("series-filter").fill("Pioneers");
  const card = page.getByTestId("series-card").first();
  await expect(card).toBeVisible();
  await card.click();

  await expect(page).toHaveURL(/\/series\/\d+$/);
});

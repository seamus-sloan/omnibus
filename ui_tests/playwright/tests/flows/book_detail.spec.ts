import { expect, test } from "../fixtures/test";
import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { fetchBookIdByTitle, getRow } from "../utils/ebooks";
import { gotoReady } from "../utils/nav";
import { fixturesDir, seedLibrary } from "../utils/seed";

// Re-seed in this spec's beforeAll so the running server is indexed against
// the committed EPUB fixtures before any assertion runs — independent of
// whatever other specs in the same worker did before us.
test.beforeAll(async ({ request }) => {
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
});

// A fixture with predictable, distinctive metadata to drive both tests.
const TARGET = FIXTURE_BOOKS.find((b) => b.slug === "alpha")!;

test("navigates from a landing row to the detail page and back", async ({ page }) => {
  await gotoReady(page, "/");

  // Click the row's cover cell to follow the SPA navigation. We target the
  // cover specifically because the seeded admin sees inline-editable cells
  // (title, author, …) that intercept clicks to open their editor instead of
  // navigating; the cover cell is non-editable, so its click bubbles to the
  // row's navigate handler — what a user clicking a non-interactive part of
  // the row gets.
  await getRow(page, TARGET.slug).getByTestId("ebook-cell-cover").click();
  // `/books/:uuid` — UUIDv5 in canonical 8-4-4-4-12 hyphenated form.
  await expect(page).toHaveURL(/\/books\/[0-9a-fA-F-]{36}$/);

  // The detail page should render the standard "Book #<id>" heading and the
  // shared back-to-library affordance.
  await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
  const backLink = page.getByRole("link", { name: "Back to library" });
  await expect(backLink).toBeVisible();

  // The back link must return us to the landing route, not just visually
  // re-render — assert URL plus that the table comes back.
  await backLink.click();
  await expect(page).toHaveURL(/\/$/);
  await expect(page.getByTestId("ebook-table")).toBeVisible();
});

test("renders the detail contents for the selected book", async ({ page, request }) => {
  // Resolve the backend id the same way a real click would: read it out of
  // the same RPC the landing page consumes. Deep-linking by id keeps this
  // test independent of the landing page's row order.
  const id = await fetchBookIdByTitle(request, TARGET.title);

  await gotoReady(page, `/books/${id}`);

  // Title heading matches the fixture
  await expect(page.getByRole("heading", { level: 1, name: TARGET.title })).toBeVisible();

  // At least the first author is visible. Scoped to the dedicated authors
  // line because the breadcrumb falls back to the first author when the book
  // has no series, so a bare getByText(...) matches twice.
  await expect(page.getByTestId("book-authors")).toContainText(TARGET.authors[0]);

  // Breadcrumb navigation: "Home" link must be present inside the breadcrumb nav
  await expect(
    page.getByRole("navigation", { name: "breadcrumb" }).getByRole("link", { name: "Home" }),
  ).toBeVisible();

  // Format switcher renders one row per available format (F1.4).
  const switcher = page.getByTestId("format-switcher");
  await expect(switcher).toBeVisible();

  // All fixture books are EPUB; the EPUB row must exist with its badge and
  // the per-format CTAs grouped underneath. Use getByTestId rather than CSS
  // attribute/class locators per `04-playwright.md` ("semantic first").
  const epubRow = switcher.getByTestId("format-row-epub");
  await expect(epubRow).toBeVisible();
  await expect(epubRow.getByTestId("format-badge")).toHaveText("EPUB");

  // Read now routes into the F2.2 immersive reader (an enabled link to
  // /read/:uuid on web); Send-to-Kindle stays disabled until F4.x ships.
  const readBtn = epubRow.getByTestId("action-read");
  await expect(readBtn).toBeVisible();
  await expect(readBtn).toHaveAttribute("href", /\/read\//);
  const kindleBtn = epubRow.getByTestId("action-kindle");
  await expect(kindleBtn).toBeVisible();
  await expect(kindleBtn).toBeDisabled();

  // No M4B fixture in the seed — the Listen CTA must NOT render.
  await expect(page.getByTestId("action-listen")).toHaveCount(0);

  // F3.2 / F3.3 placeholder slots must be in the DOM (may be invisible)
  await expect(page.getByTestId("ratings-slot")).toBeAttached();
  await expect(page.getByTestId("suggestions-slot")).toBeAttached();

  // Back link still navigates to landing
  const backLink = page.getByRole("link", { name: "Back to library" });
  await expect(backLink).toBeVisible();
  await backLink.click();
  await expect(page).toHaveURL(/\/$/);
  await expect(page.getByTestId("ebook-table")).toBeVisible();
});

test("breadcrumb author segment links to the author page", async ({ page, request }) => {
  // `alpha` has a single author (Ada Lovelace) and no series, so the
  // breadcrumb shape is Home > Ada Lovelace > Alpha and the author segment
  // must be a router link to /authors/:id.
  const id = await fetchBookIdByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${id}`);

  const crumb = page.getByRole("navigation", { name: "breadcrumb" });
  const authorLink = crumb.getByRole("link", { name: TARGET.authors[0] });
  await expect(authorLink).toBeVisible();
  await authorLink.click();
  await expect(page).toHaveURL(/\/authors\/\d+$/);
});

test("breadcrumb series segment links to the series page", async ({ page, request }) => {
  // `beta` is "Beta in the Series" #1 of "Pioneers" — so the breadcrumb is
  // Home > Grace Hopper > Pioneers #1 > Beta in the Series. The series
  // segment must be a router link to /series/:id.
  const beta = FIXTURE_BOOKS.find((b) => b.slug === "beta")!;
  const id = await fetchBookIdByTitle(request, beta.title);
  await gotoReady(page, `/books/${id}`);

  const crumb = page.getByRole("navigation", { name: "breadcrumb" });
  const seriesLink = crumb.getByRole("link", { name: /Pioneers/ });
  await expect(seriesLink).toBeVisible();
  await seriesLink.click();
  await expect(page).toHaveURL(/\/series\/\d+$/);
});

import { expect, test } from "../fixtures/test";
import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { fetchBookIdByTitle, getRow } from "../utils/ebooks";
import { gotoReady } from "../utils/nav";
import { fixturesDir, seedLibrary } from "../utils/seed";

// Re-seed in this spec's beforeAll. Each Playwright test file runs in its own
// worker, so it can't rely on `landing.spec.ts`'s seed having run.
test.beforeAll(async ({ request }) => {
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
});

// A fixture with predictable, distinctive metadata to drive both tests.
const TARGET = FIXTURE_BOOKS.find((b) => b.slug === "alpha")!;

test("navigates from a landing row to the detail page and back", async ({ page }) => {
  await gotoReady(page, "/");

  // Click the row for our target book and follow the SPA navigation.
  await getRow(page, TARGET.slug).click();
  await expect(page).toHaveURL(/\/books\/\d+$/);

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

  // At least the first author is visible
  await expect(page.getByText(TARGET.authors[0])).toBeVisible();

  // Breadcrumb navigation: "Home" link must be present inside the breadcrumb nav
  await expect(
    page.getByRole("navigation", { name: "breadcrumb" }).getByRole("link", { name: "Home" }),
  ).toBeVisible();

  // All fixture books are EPUB — "Read" CTA must be rendered
  await expect(page.getByTestId("action-read")).toBeVisible();

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

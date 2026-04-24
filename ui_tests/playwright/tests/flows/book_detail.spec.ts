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

  await expect(page.getByRole("heading", { level: 1, name: `Book #${id}` })).toBeVisible();
  // Today the detail page is a stub. Pin the placeholder copy so when the
  // real metadata view lands, this test fails loudly and forces us to
  // rewrite it against the new contract instead of silently passing.
  await expect(page.getByText("Book detail page — TODO.")).toBeVisible();
  await expect(page.getByRole("link", { name: "Back to library" })).toBeVisible();
});

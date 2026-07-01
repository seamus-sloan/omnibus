import { expect, test } from "../fixtures/test";
import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { expectNavVisible, gotoReady } from "../utils/nav";
import { expectRowMatches } from "../utils/ebooks";
import { fixturesDir, seedLibrary } from "../utils/seed";

// Seed the running server against the committed EPUB fixtures before any
// landing-page assertion runs. The settings POST kicks off an async reindex
// inside the server (`tokio::spawn`), so `seedLibrary` polls
// `/api/rpc/ebooks` until the indexer has surfaced every fixture.
test.beforeAll(async ({ request }) => {
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
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
  // F3.1: the shelves rail replaced the old filter sidebar. "All books" is the
  // way back to the full library; shelf CRUD is covered by shelves.spec.ts.
  await expect(page.getByTestId("rail-all-books")).toBeVisible();
  await expect(page.getByTestId("new-shelf")).toBeVisible();
  await expectNavVisible(page);
});

test("renders every fixture book with the expected metadata", async ({ page }) => {
  await gotoReady(page, "/");

  // Every ebook fixture must appear; audiobooks may also be present when
  // parallel specs have seeded the audiobook library on the shared server.
  const rowCount = await page.getByTestId(/^ebook-row-/).count();
  expect(rowCount).toBeGreaterThanOrEqual(FIXTURE_BOOKS.length);

  for (const expected of FIXTURE_BOOKS) {
    await test.step(`renders "${expected.title}" from ${expected.filename}`, async () => {
      await expectRowMatches(page, expected);
    });
  }
});

test("browse fits the fixture library in one keyset page (no Load more)", async ({ page }) => {
  await gotoReady(page, "/");

  // F5b: browse is keyset-paginated, but the fixtures are far under the
  // 100-book page size, so the first page contains the whole library and the
  // pagination sentinel must not render. (Above the page size, a
  // `lib-load-more` button appears and appends further pages.)
  await expect(page.getByTestId(/^ebook-row-/).first()).toBeVisible();
  const rowCount = await page.getByTestId(/^ebook-row-/).count();
  expect(rowCount).toBeGreaterThanOrEqual(FIXTURE_BOOKS.length);
  await expect(page.getByTestId("lib-load-more")).toHaveCount(0);
});

test("toggles to grid view and persists across reload", async ({ page }) => {
  await gotoReady(page, "/");

  await expect(page.getByTestId("ebook-table")).toBeVisible();
  await page.getByTestId("view-toggle-grid").click();

  await expect(page.getByTestId("lib-grid")).toBeVisible();
  await expect(page.getByTestId("ebook-table")).toHaveCount(0);
  const tileCount = await page.getByTestId(/^ebook-tile-/).count();
  expect(tileCount).toBeGreaterThanOrEqual(FIXTURE_BOOKS.length);
  await expect(page.getByTestId("view-toggle-grid")).toHaveAttribute("aria-pressed", "true");

  await page.reload();
  await page.waitForLoadState("networkidle");
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

// F3.1 removed the home-page facet filters (format chips + author/series/tag
// sidebar); their coverage retired with them. Slicing the library is now the
// job of shelves — see shelves.spec.ts.

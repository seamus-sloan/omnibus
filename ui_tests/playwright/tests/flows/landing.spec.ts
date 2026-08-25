import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { expect, test } from "../fixtures/test";
import { expectRowMatches, switchToTableView } from "../utils/ebooks";
import { expectNavVisible, gotoReady } from "../utils/nav";
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

  await expect(
    page.getByRole("heading", { level: 1, name: "Your Library" }),
  ).toBeVisible();
  // Grid is the default view mode; the table is opt-in via the toolbar.
  await expect(page.getByTestId("lib-grid")).toBeVisible();
  await expect(page.getByTestId("lib-toolbar")).toBeVisible();
  // The shelves row replaced the left rail on the landing: an "All Books"
  // entry (selected by default) plus a New-shelf action, with paging buttons
  // that `marquee.js` arms only at an end that has more to show. In-place
  // selection and paging are covered by shelf_gallery.spec.ts; shelf CRUD by
  // shelves.spec.ts.
  await expect(page.getByTestId("shelf-gallery")).toBeVisible();
  await expect(page.getByTestId("gallery-all-books")).toBeVisible();
  await expect(page.getByTestId("new-shelf")).toBeVisible();
  await expect(page.getByTestId("shelf-row-prev")).toBeAttached();
  await expect(page.getByTestId("shelf-row-next")).toBeAttached();
  await expectNavVisible(page);
});

test("the cover wall keeps its captions off the covers until hover", async ({
  page,
}) => {
  await gotoReady(page, "/");

  // Read-only: any book will do, so this takes the first tile rather than
  // reserving a fixture.
  const tile = page.getByTestId(/^ebook-tile-/).first();
  await expect(tile).toBeVisible();
  const caption = tile.locator(".lib-tile-cap");

  // The wall is covers; the words are a layer over the cover's foot that is
  // transparent until pointed at. `toBeHidden` would not catch this — an
  // opacity-0 element is still "visible" to Playwright.
  await expect(caption).toHaveCSS("opacity", "0");
  await tile.hover();
  await expect(caption).toHaveCSS("opacity", "1");
});

test("renders every fixture book with the expected metadata", async ({
  page,
}) => {
  await gotoReady(page, "/");
  await switchToTableView(page);

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

test("browse fits the fixture library in one keyset page (no Load more)", async ({
  page,
}) => {
  await gotoReady(page, "/");

  // F5b: browse is keyset-paginated, but the fixtures are far under the
  // 100-book page size, so the first page contains the whole library and the
  // pagination sentinel must not render. (Above the page size, a
  // `lib-load-more` button appears and appends further pages.)
  await expect(page.getByTestId(/^ebook-tile-/).first()).toBeVisible();
  const tileCount = await page.getByTestId(/^ebook-tile-/).count();
  expect(tileCount).toBeGreaterThanOrEqual(FIXTURE_BOOKS.length);
  await expect(page.getByTestId("lib-load-more")).toHaveCount(0);
});

test("toggles to table view and persists across reload", async ({ page }) => {
  await gotoReady(page, "/");

  await expect(page.getByTestId("lib-grid")).toBeVisible();
  await page.getByTestId("view-toggle-table").click();

  await expect(page.getByTestId("ebook-table")).toBeVisible();
  await expect(page.getByTestId("lib-grid")).toHaveCount(0);
  const rowCount = await page.getByTestId(/^ebook-row-/).count();
  expect(rowCount).toBeGreaterThanOrEqual(FIXTURE_BOOKS.length);
  await expect(page.getByTestId("view-toggle-table")).toHaveAttribute(
    "aria-pressed",
    "true",
  );

  await page.reload();
  await page.waitForLoadState("networkidle");
  await expect(page.getByTestId("ebook-table")).toBeVisible();
  await expect(page.getByTestId("lib-grid")).toHaveCount(0);
});

test("sorts by title descending when the Title header is clicked", async ({
  page,
}) => {
  await gotoReady(page, "/");
  await switchToTableView(page);

  // Default sort is title asc — click once to flip to desc.
  await page.getByRole("button", { name: /^Title( ↑| ↓)?$/ }).click();

  const titleHeader = page.locator(".sort-th[aria-sort='descending']");
  await expect(titleHeader).toBeVisible();

  // First row's title cell should match the alphabetically-last fixture.
  const titles = [...FIXTURE_BOOKS.map((b) => b.title)].sort((a, b) =>
    a.toLowerCase().localeCompare(b.toLowerCase()),
  );
  const lastTitle = titles[titles.length - 1]!;
  await expect(
    page
      .getByTestId(/^ebook-row-/)
      .first()
      .getByTestId("ebook-cell-title"),
  ).toHaveText(lastTitle);
});

// F3.1 removed the home-page facet filters (format chips + author/series/tag
// sidebar); their coverage retired with them. Slicing the library is now the
// job of shelves — see shelves.spec.ts.

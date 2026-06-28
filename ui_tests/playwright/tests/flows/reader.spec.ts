import { expect, test } from "../fixtures/test";
import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { expectMutation } from "../utils/api";
import { fetchBookUuidByTitle } from "../utils/ebooks";
import { gotoReady } from "../utils/nav";
import { fixturesDir, seedLibrary } from "../utils/seed";

// Re-seed in this spec's beforeAll so the running server is indexed against
// the committed EPUB fixtures before any assertion runs — independent of
// whatever other specs in the same worker did before us.
test.beforeAll(async ({ request }) => {
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
});

// A fixture with predictable, distinctive metadata to drive most tests.
const TARGET = FIXTURE_BOOKS.find((b) => b.slug === "alpha")!;
// A separate book for the highlight action test so seeding/deleting highlights
// doesn't pollute the empty-state assertions on TARGET (tests run in parallel).
const HL_BOOK = FIXTURE_BOOKS.find((b) => b.slug === "beta")!;

test("renders the reader layout", async ({ page, request }) => {
  // Deep-link straight to the immersive reader by the book's stable uuid,
  // resolved the same way a real click would.
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await gotoReady(page, `/read/${uuid}`);

  // The immersive reader mounts a viewer container plus chrome bars. We
  // assert only on the chrome — never on rendered EPUB text — because the
  // epub.js render is JS-engine-driven and may not paint headlessly (and the
  // book file may be absent in some seed states). The container/controls are
  // SSR markup and are robust to that.
  await expect(page.getByTestId("reader-viewer")).toBeVisible();

  // Top chrome: back, contents, search, Aa, highlights, bookmark.
  await expect(page.getByTestId("reader-back")).toBeVisible();
  await expect(page.getByTestId("reader-toc")).toBeVisible();
  await expect(page.getByTestId("reader-search")).toBeVisible();
  await expect(page.getByTestId("reader-aa")).toBeVisible();
  await expect(page.getByTestId("reader-highlights")).toBeVisible();
  await expect(page.getByTestId("reader-bookmark")).toBeVisible();

  // Page-turn gutters.
  await expect(page.getByTestId("reader-prev")).toBeVisible();
  await expect(page.getByTestId("reader-next")).toBeVisible();

  // Font + page-view controls are inside the Aa panel — open it first.
  await page.getByTestId("reader-aa").click();
  await expect(page.getByTestId("reader-font-decrease")).toBeVisible();
  await expect(page.getByTestId("reader-font-increase")).toBeVisible();
  await expect(page.getByTestId("reader-spread-single")).toBeVisible();
  await expect(page.getByTestId("reader-spread-double")).toBeVisible();
});

test("opens the search and bookmarks drawers from the top chrome", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await gotoReady(page, `/read/${uuid}`);
  await expect(page.getByTestId("reader-viewer")).toBeVisible();

  // Search drawer opens with its query input visible.
  await page.getByTestId("reader-search").click();
  await expect(page.getByTestId("reader-search-drawer")).toBeVisible();
  await expect(page.getByTestId("reader-search-input")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("reader-search-drawer")).toHaveCount(0);

  // Bookmarks drawer: empty state + the "+ Bookmark" affordance.
  await page.getByTestId("reader-bookmark").click();
  await expect(page.getByTestId("reader-bookmarks-drawer")).toBeVisible();
  await expect(page.getByTestId("reader-bookmark-add")).toBeVisible();
});

test("opens the contents and highlights drawers from the top chrome", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await gotoReady(page, `/read/${uuid}`);
  await expect(page.getByTestId("reader-viewer")).toBeVisible();

  // Highlights drawer opens with the (empty) palette filter rail.
  await page.getByTestId("reader-highlights").click();
  await expect(page.getByTestId("reader-highlights-drawer")).toBeVisible();
  await expect(page.getByText("No highlights yet")).toBeVisible();

  // Escape peels the drawer back (its scrim otherwise covers the chrome).
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("reader-highlights-drawer")).toHaveCount(0);

  // Contents drawer opens from the top chrome.
  await page.getByTestId("reader-toc").click();
  await expect(page.getByTestId("reader-toc-drawer")).toBeVisible();
});

test("seeds a highlight and deletes it from the highlights drawer", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, HL_BOOK.title);

  // Seed a highlight via the REST API (bearer-authed request fixture) so the
  // reader loads it into the drawer on mount. A unique quote keeps the test
  // robust against the shared, persistent dev DB.
  const quote = `seeded passage ${Date.now()}`;
  const created = await request.post("/api/highlights", {
    data: {
      book_uuid: uuid,
      epub_cfi_range: "epubcfi(/6/4!/4/2,/1:0,/1:40)",
      color: "amber",
      text: quote,
    },
  });
  expect(created.status(), "seed highlight").toBe(200);

  await gotoReady(page, `/read/${uuid}`);
  await expect(page.getByTestId("reader-viewer")).toBeVisible();

  // Open the highlights drawer — the seeded highlight is listed.
  await page.getByTestId("reader-highlights").click();
  await expect(page.getByTestId("reader-highlights-drawer")).toBeVisible();
  const row = page
    .getByTestId("reader-highlight-row")
    .filter({ hasText: quote });
  await expect(row).toHaveCount(1);

  // Delete it — the delete RPC must fire, and the row disappears.
  await expectMutation(
    page,
    {
      method: "POST",
      url: /\/api\/rpc\/highlights\/delete(?:\?|$)/,
      expectedStatus: 200,
    },
    async () => row.getByTestId("highlight-delete").click(),
  );
  await expect(page.getByText(quote)).toHaveCount(0);
});

test("opens the reader from the book detail Read action", async ({ page, request }) => {
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${uuid}`);

  // The EPUB "Read" CTA in the format switcher routes into the immersive
  // reader. This is a read-only client-side navigation (no mutation), so we
  // follow the SPA route change and assert on the destination chrome.
  await page.getByTestId("action-read").click();
  await expect(page).toHaveURL(new RegExp(`/read/${uuid}$`));
  await expect(page.getByTestId("reader-viewer")).toBeVisible();
});

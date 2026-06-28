import { expect, test } from "../fixtures/test";
import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { fetchBookUuidByTitle } from "../utils/ebooks";
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

  // Top chrome: back, contents, Aa display settings, highlights, bookmark.
  await expect(page.getByTestId("reader-back")).toBeVisible();
  await expect(page.getByTestId("reader-toc")).toBeVisible();
  await expect(page.getByTestId("reader-aa")).toBeVisible();
  await expect(page.getByTestId("reader-highlights")).toBeVisible();
  await expect(page.getByTestId("reader-bookmark")).toBeVisible();

  // Page-turn gutters.
  await expect(page.getByTestId("reader-prev")).toBeVisible();
  await expect(page.getByTestId("reader-next")).toBeVisible();

  // Font controls are inside the Aa panel — open it first.
  await page.getByTestId("reader-aa").click();
  await expect(page.getByTestId("reader-font-decrease")).toBeVisible();
  await expect(page.getByTestId("reader-font-increase")).toBeVisible();
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

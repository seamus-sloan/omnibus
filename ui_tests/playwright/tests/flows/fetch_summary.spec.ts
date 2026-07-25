import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { expect, test } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import { fetchBookIdByTitle, fetchBookUuidByTitle } from "../utils/ebooks";
import { gotoReady } from "../utils/nav";
import { fixturesDir, seedLibrary } from "../utils/seed";

// The "Fetch Summary" button pulls a book blurb from Hardcover/OpenLibrary on
// demand. The generated fixtures ship no descriptions, so every fixture book
// is "sparse" (< 10 words) — the condition under which the detail-page button
// appears. All three external endpoints are mocked via `page.route` so CI
// never touches a live Hardcover/OpenLibrary API.

const TARGET = FIXTURE_BOOKS.find((b) => b.slug === "alpha")!;

test.beforeAll(async ({ request }) => {
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
  // Clear any description override on the target so it starts sparse (the
  // detail button's precondition) and its editor field starts empty —
  // hermetic regardless of overrides a prior run/manual test may have left.
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await request.delete(`/api/ebooks/${uuid}/overrides`);
});

const CONFIGURED_URL = "**/api/rpc/ebook/summary/hardcover-configured";
const FETCH_URL = "**/api/rpc/ebook/summary/fetch";

/** Await the summary-fetch POST that a button click fires, so DOM assertions
 * don't race the network (per `.claude/rules/04-playwright.md`). */
function waitForFetch(page: Parameters<typeof gotoReady>[0]) {
  return page.waitForResponse(
    (r) =>
      r.url().includes("/api/rpc/ebook/summary/fetch") &&
      r.request().method() === "POST",
  );
}

/** Mock the Hardcover-configured flag (drives which source the cascade starts on). */
async function mockConfigured(
  page: Parameters<typeof gotoReady>[0],
  configured: boolean,
) {
  await page.route(CONFIGURED_URL, (route) =>
    route.request().method() === "POST"
      ? route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify(configured),
        })
      : route.continue(),
  );
}

/** Mock the summary fetch. `text` → `Ok(Some(text))`; `null` → `Ok(None)` (a miss). */
async function mockFetch(
  page: Parameters<typeof gotoReady>[0],
  text: string | null,
) {
  await page.route(FETCH_URL, (route) =>
    route.request().method() === "POST"
      ? route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify(text),
        })
      : route.continue(),
  );
}

// ---------------------------------------------------------------------------
// Editor — the button is always present, fills the field, and doesn't save.
// ---------------------------------------------------------------------------

test("metadata editor always shows the Fetch Summary button", async ({
  page,
  request,
}) => {
  const id = await fetchBookIdByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${id}/edit`);
  await expect(page.getByTestId("fetch-summary")).toBeVisible();
});

test("Fetch Summary fills the editor Description without saving", async ({
  page,
  request,
}) => {
  const id = await fetchBookIdByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${id}/edit`);

  const SUMMARY =
    "A mocked summary fetched from OpenLibrary for the editor fill test.";
  await mockConfigured(page, false); // no key → straight to OpenLibrary
  await mockFetch(page, SUMMARY);

  const fetched = waitForFetch(page);
  await page.getByTestId("fetch-summary").click();
  await fetched;

  // The Description textarea (id `me-description`) is filled with the fetched
  // text; the editor never auto-saves, so this is staged for the user's Save.
  await expect(page.locator("#me-description")).toHaveValue(SUMMARY);
  // Still on the edit form (no navigation-away that a save would trigger).
  await expect(page.getByTestId("me-save")).toBeVisible();
});

test("Fetch Summary surfaces an error when neither source has a summary", async ({
  page,
  request,
}) => {
  const id = await fetchBookIdByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${id}/edit`);

  await mockConfigured(page, false);
  await mockFetch(page, null); // OpenLibrary miss → Ok(None)

  const fetched = waitForFetch(page);
  await page.getByTestId("fetch-summary").click();
  await fetched;

  await expect(page.getByTestId("fetch-summary-error")).toHaveText(
    "No summary found.",
  );
  await expect(page.locator("#me-description")).toHaveValue("");
});

// ---------------------------------------------------------------------------
// Detail page — button only when sparse; a fetch saves + refreshes in place.
// ---------------------------------------------------------------------------

test("detail page fetch saves the summary and hides the button", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${uuid}`);

  // Sparse (empty) summary → the button is offered.
  await expect(page.getByTestId("fetch-summary")).toBeVisible();

  // 14 words, so once saved the summary is no longer sparse and the button hides.
  const SUMMARY =
    "A sufficiently long mocked summary with more than ten words to enrich the book.";
  await mockConfigured(page, false);
  await mockFetch(page, SUMMARY);
  // Mock the override save so no real DB write leaks across specs; `null` decodes
  // as `Ok(None)`, which the client treats as a successful save.
  await page.route("**/api/rpc/ebook/overrides", (route) =>
    route.request().method() === "POST"
      ? route.fulfill({
          status: 200,
          contentType: "application/json",
          body: "null",
        })
      : route.continue(),
  );

  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/ebook/overrides",
      expectedBody: { uuid, overrides: { description: SUMMARY } },
      expectedStatus: 200,
    },
    async () => page.getByTestId("fetch-summary").click(),
  );

  await expect(page.getByTestId("book-description")).toContainText(SUMMARY);
  await expect(page.getByTestId("fetch-summary")).toBeHidden();
});

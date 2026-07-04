import type { Page } from "@playwright/test";
import { expect, test } from "../fixtures/test";
import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { fetchBookUuidByTitle } from "../utils/ebooks";
import { gotoReady } from "../utils/nav";
import { fixturesDir, seedLibrary } from "../utils/seed";

// F3.3 "Readers also enjoyed" strip on the book-detail page. The strip's
// state depends on whether a Hardcover key is configured server-wide and on
// the outcome of a real Hardcover API call — neither of which this suite can
// control deterministically (no `HARDCOVER_API_KEY` in CI, and the key is
// shared global state other specs, e.g. `settings.spec.ts`, also mutate). So
// every test here mocks `POST /api/rpc/ebook-suggestions` via `page.route`
// rather than seeding real `book_suggestions` rows — the frontend calls a
// plain interceptable server function (see `rpc_get_suggestions` in
// `frontend/src/rpc/books.rs`), so this reproduces every UI state exactly
// without a new DB seeding seam or any cross-spec state race.
test.beforeAll(async ({ request }) => {
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
});

const TARGET = FIXTURE_BOOKS.find((b) => b.slug === "alpha")!;

interface MockSuggestion {
  hardcover_id: number;
  hardcover_url: string;
  title: string;
  author: string;
  list_count: number;
  cover_url: string | null;
}

function mockSuggestion(rank: number): MockSuggestion {
  return {
    hardcover_id: 1000 + rank,
    hardcover_url: `https://hardcover.app/books/mock-${rank}`,
    title: `Suggested Book ${rank}`,
    author: `Suggested Author ${rank}`,
    list_count: rank + 1,
    cover_url: null,
  };
}

/** Intercept the suggestions fetch and reply with a fixed `SuggestionsResponse`. */
async function mockSuggestionsResponse(page: Page, body: unknown): Promise<void> {
  await page.route("**/api/rpc/ebook-suggestions", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(body),
    }),
  );
}

/**
 * Navigate to a book's detail page and wait for the suggestions strip's
 * POST /api/rpc/ebook-suggestions to resolve (it's a POST per Dioxus server
 * function convention, and the handler enqueues a cascade resolution as a
 * side effect — not a side-effect-free read), so the page has settled into
 * whichever mocked state before assertions run.
 *
 * This deliberately doesn't reuse `expectMutation`: that helper's request
 * wait is tuned for a click-triggered mutation on an already-hydrated page,
 * but this request fires from the page's own mount effect right after WASM
 * hydration — which, on a fresh navigation, can take meaningfully longer
 * than a warm in-page action.
 */
async function gotoBookAwaitingSuggestions(page: Page, uuid: string): Promise<void> {
  const requestPromise = page.waitForRequest(
    (r) => r.method() === "POST" && r.url().includes("/api/rpc/ebook-suggestions"),
    { timeout: 30_000 },
  );
  await gotoReady(page, `/books/${uuid}`);
  const request = await requestPromise;
  const response = await request.response();
  expect(response?.status()).toBe(200);
}

// ---------------------------------------------------------------------------
// State — not configured (no Hardcover key)
// ---------------------------------------------------------------------------

test("shows a connect prompt when no Hardcover key is configured", async ({ page, request }) => {
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await mockSuggestionsResponse(page, { status: "not_configured" });
  await gotoBookAwaitingSuggestions(page, uuid);

  const strip = page.getByTestId("suggestions-strip");
  await expect(strip).toBeVisible();
  await expect(page.getByTestId("suggestions-not-configured")).toBeVisible();
  await expect(
    page.getByText("Add a Hardcover API key to pull read-alikes for this book."),
  ).toBeVisible();
  // The seeded dev user is an admin, so the connect card links to Settings.
  const connectLink = page.getByTestId("suggestions-connect-link");
  await expect(connectLink).toBeVisible();
  await expect(connectLink).toHaveAttribute("href", "/settings");
});

// ---------------------------------------------------------------------------
// State — pending (resolution enqueued, nothing cached yet)
// ---------------------------------------------------------------------------

test("shows a pending placeholder while a resolution is enqueued", async ({ page, request }) => {
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await mockSuggestionsResponse(page, { status: "pending" });
  await gotoBookAwaitingSuggestions(page, uuid);

  await expect(page.getByTestId("suggestions-pending")).toBeVisible();
  await expect(page.getByText("Looking for read-alikes via Hardcover…")).toBeVisible();
});

// ---------------------------------------------------------------------------
// State — ready, no matches (sticky negative)
// ---------------------------------------------------------------------------

test("shows an empty note when the cache resolved with no matches", async ({ page, request }) => {
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await mockSuggestionsResponse(page, { status: "ready", items: [] });
  await gotoBookAwaitingSuggestions(page, uuid);

  await expect(page.getByTestId("suggestions-empty")).toBeVisible();
  await expect(page.getByText("No read-alikes found for this book yet.")).toBeVisible();
});

// ---------------------------------------------------------------------------
// State — ready, with results (collapsed stack + "show more" + outbound link)
// ---------------------------------------------------------------------------

test("renders cached suggestions and expands the collapsed stack via show more", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  const items = Array.from({ length: 7 }, (_, i) => mockSuggestion(i));
  await mockSuggestionsResponse(page, { status: "ready", items });
  await gotoBookAwaitingSuggestions(page, uuid);

  const strip = page.getByTestId("suggestions-strip");
  const cards = strip.getByTestId("suggestion-card");

  // Collapsed stack caps at 5 even though 7 items are cached.
  await expect(cards).toHaveCount(5);
  await expect(cards.first()).toContainText(items[0]!.title);
  await expect(cards.first()).toContainText(items[0]!.author);
  await expect(cards.first()).toContainText("on 1 list");

  // Each card is an outbound link to the Hardcover book page, opened in a
  // new tab without leaking a `window.opener` reference.
  await expect(cards.first()).toHaveAttribute("href", items[0]!.hardcover_url);
  await expect(cards.first()).toHaveAttribute("target", "_blank");
  await expect(cards.first()).toHaveAttribute("rel", "noopener noreferrer");

  const showMore = page.getByTestId("suggestions-show-more");
  await expect(showMore).toHaveText("Show 2 more");
  await showMore.click();

  await expect(cards).toHaveCount(7);
  await expect(cards.last()).toContainText(items[6]!.title);
  await expect(showMore).toHaveCount(0);
});

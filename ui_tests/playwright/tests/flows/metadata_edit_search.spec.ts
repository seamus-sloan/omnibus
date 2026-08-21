import type { Page } from "@playwright/test";
import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { expect, test } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import { fetchBookIdByTitle } from "../utils/ebooks";
import { gotoReady } from "../utils/nav";
import { fixturesDir, seedLibrary } from "../utils/seed";

// The edition picker on the metadata-edit page: a two-screen overlay that
// searches every configured provider (#1661), shows what one source would
// change (#1662), and applies its cover (#1663).
//
// The flow searches as it opens — the query is the book's own title and
// author, so there is nothing to ask first — which is why every test installs
// its provider mocks *before* clicking the trigger.
//
// Every provider response is stubbed via `page.route`. The suite must never
// depend on a live provider or a configured key — the same reasoning
// `fetch_summary.spec.ts` gives, and doubly so here, since a
// fan-out would otherwise spend three real API calls per test.
//
// `mathematica-minor-2` ("Minor Lemmas II", Sophie Germain) is read by no
// other spec — see the fixture-isolation note in
// `.claude/rules/04-playwright.md`. That reservation is load-bearing: one
// test saves a real override on it and reverts it, and an `afterAll` clears
// any override a failure left behind. (`author_delete.spec.ts` opens a modal
// on this book's author, but only regex-matches a book count and cancels.)

test.beforeAll(async ({ request }) => {
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
});

// Unconditional: the save test reverts on its happy path, but a failure
// between the save and the revert would otherwise leave a persistent override
// on a shared fixture for every later run.
test.afterAll(async ({ request }) => {
  const uuid = await fetchBookIdByTitle(request, TARGET.title);
  await request.post("/api/rpc/ebook/overrides/delete", { data: { uuid } });
});

const TARGET = FIXTURE_BOOKS.find((b) => b.slug === "mathematica-minor-2")!;

const PROVIDERS_URL = "**/api/rpc/metadata/providers";
const SEARCH_URL = "**/api/rpc/metadata/editions/search";
const HYDRATE_URL = "**/api/rpc/metadata/editions/hydrate";
const COVER_FROM_URL = "**/api/ebooks/*/cover/from-url";

// A 1x1 transparent GIF, so a candidate row's cover renders without the
// browser reaching a real provider host during the test.
const PIXEL =
  "data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7";

const CAPABILITIES = {
  search_by_title: true,
  search_by_isbn: true,
  carries_cover: true,
  carries_ratings: false,
  carries_genres: true,
};

type ProviderId = "open_library" | "google_books" | "hardcover";

function provider(id: ProviderId, displayName: string, configured: boolean) {
  return {
    id,
    display_name: displayName,
    configured,
    requires_key: id === "hardcover",
    capabilities: CAPABILITIES,
  };
}

const ALL_CONFIGURED = [
  provider("open_library", "Open Library", true),
  provider("google_books", "Google Books", true),
  provider("hardcover", "Hardcover", false),
];

// Two candidates sharing an ISBN across two sources: the picker's job is to
// keep two sources' takes on one edition apart. Both titles account for every
// word of the seeded query, so the shorter one leads — whichever provider
// answered first, and whether or not the other answered at all.
const OL_CANDIDATE = {
  source: "open_library",
  provider_ref: "/works/OL42W",
  isbn13: "9781234567897",
  title: "Minor Lemmas II (Open Library)",
  authors: ["Sophie Germain"],
  year: "1815",
  pages: 212,
  publisher: "Klein Mathematik",
  description: null,
  cover_url: PIXEL,
  series: null,
  series_index: null,
  first_publish_year: 1815,
  genres: ["Mathematics"],
};

const GB_CANDIDATE = {
  source: "google_books",
  provider_ref: "gb-volume-7",
  isbn13: "9781234567897",
  title: "Minor Lemmas II: A Longer Google Books Title",
  authors: ["Sophie Germain", "Emmy Noether"],
  year: "1816",
  pages: 220,
  publisher: "Editions Gauss",
  description: "A Google Books description.",
  cover_url: PIXEL,
  series: null,
  series_index: null,
  first_publish_year: null,
  genres: [],
};

const SOURCES = [
  {
    provider: "open_library",
    display_name: "Open Library",
    status: { kind: "answered", count: 1 },
  },
  {
    provider: "google_books",
    display_name: "Google Books",
    status: { kind: "answered", count: 1 },
  },
  {
    provider: "hardcover",
    display_name: "Hardcover",
    status: { kind: "not_configured" },
  },
];

// Deliberately provider-first order, the shape the fan-out actually returns —
// so a test asserting row order is asserting the client's sort, not the
// server's concatenation.
const FOUND = { editions: [GB_CANDIDATE, OL_CANDIDATE], sources: SOURCES };

// The AC2 fixture: a Hardcover candidate, the only source that publishes a
// series position. Kept apart from FOUND because every other test in this
// file runs with Hardcover unconfigured.
const HC_CANDIDATE = {
  source: "hardcover",
  provider_ref: "hc-book-31",
  isbn13: "9789999888877",
  title: "Minor Lemmas II (Hardcover)",
  authors: ["Sophie Germain", "Sofya Kovalevskaya"],
  year: "1817",
  pages: 244,
  publisher: "Hardcover Press",
  description: "A Hardcover description.",
  cover_url: PIXEL,
  series: "Mathematica Majora",
  series_index: "7",
  first_publish_year: 1815,
  genres: ["Mathematics"],
};

const HC_CONFIGURED = [
  provider("open_library", "Open Library", true),
  provider("google_books", "Google Books", true),
  provider("hardcover", "Hardcover", true),
];

const HC_FOUND = {
  editions: [HC_CANDIDATE],
  sources: [
    {
      provider: "open_library",
      display_name: "Open Library",
      status: { kind: "answered", count: 0 },
    },
    {
      provider: "google_books",
      display_name: "Google Books",
      status: { kind: "answered", count: 0 },
    },
    {
      provider: "hardcover",
      display_name: "Hardcover",
      status: { kind: "answered", count: 1 },
    },
  ],
};

async function mockProviders(
  page: Page,
  catalog: ReturnType<typeof provider>[],
): Promise<void> {
  await page.route(PROVIDERS_URL, (route) =>
    route.request().method() === "GET"
      ? route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify(catalog),
        })
      : route.continue(),
  );
}

async function mockSearch(page: Page, body: unknown): Promise<void> {
  await page.route(SEARCH_URL, (route) =>
    route.request().method() === "POST"
      ? route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify(body),
        })
      : route.continue(),
  );
}

async function failSearch(page: Page): Promise<void> {
  await page.route(SEARCH_URL, (route) =>
    route.request().method() === "POST"
      ? route.fulfill({
          status: 500,
          contentType: "text/plain",
          body: "provider config unavailable",
        })
      : route.continue(),
  );
}

/** Hydrate answers `null` unless a test says otherwise — the picker keeps the
 * candidate it already has, which is what most tests want. */
async function mockHydrate(page: Page, body: unknown = null): Promise<void> {
  await page.route(HYDRATE_URL, (route) =>
    route.request().method() === "POST"
      ? route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify(body),
        })
      : route.continue(),
  );
}

/** Open the overlay and wait out the search it fires on open. */
async function openPicker(page: Page, uuid: string): Promise<void> {
  await gotoReady(page, `/books/${uuid}/edit`);
  const searched = page.waitForResponse(
    (r) =>
      r.url().includes("/api/rpc/metadata/editions/search") &&
      r.request().method() === "POST",
  );
  await page.getByTestId("metadata-search-btn").click();
  await searched;
}

/** Open the overlay on the results screen with every provider configured. */
async function openResults(page: Page, uuid: string): Promise<void> {
  await mockProviders(page, ALL_CONFIGURED);
  await mockHydrate(page);
  await mockSearch(page, FOUND);
  await openPicker(page, uuid);
  await expect(page.getByTestId("mes-candidates")).toBeVisible();
}

/** Open the compare screen for the Google Books candidate, which answers for
 * a description and a publisher but has no series. */
async function openCompare(page: Page, uuid: string): Promise<void> {
  await openResults(page, uuid);
  // Sorted by title, so Google Books' "Zebra…" is the second row whichever
  // order the fan-out returned them in.
  await page.getByTestId("mes-candidate-1").click();
  await expect(page.getByTestId("mes-compare")).toBeVisible();
}

test.describe
  .serial("metadata edit edition picker", () => {
    // -------------------------------------------------------------------------
    // Layout — the trigger, and an overlay that arrives at answers
    // -------------------------------------------------------------------------

    test("opens an overlay that has already searched for this book", async ({
      page,
      request,
    }) => {
      const uuid = await fetchBookIdByTitle(request, TARGET.title);
      await mockProviders(page, ALL_CONFIGURED);
      await mockHydrate(page);
      await mockSearch(page, FOUND);
      await gotoReady(page, `/books/${uuid}/edit`);

      const trigger = page.getByTestId("metadata-search-btn");
      await expect(trigger).toBeVisible();
      await expect(page.getByTestId("metadata-search")).toHaveCount(0);

      const searched = page.waitForResponse(
        (r) =>
          r.url().includes("/api/rpc/metadata/editions/search") &&
          r.request().method() === "POST",
      );
      await trigger.click();
      await searched;

      // Prefilled from the book, and already run — the reader lands on results,
      // not on a form asking the question they just asked.
      await expect(page.getByTestId("metadata-search")).toBeVisible();
      await expect(page.getByTestId("mes-query")).toHaveValue(
        `${TARGET.title} ${TARGET.authors[0]}`,
      );
      await expect(page.getByTestId("mes-candidates")).toBeVisible();
    });

    test("closes on the close button and on a click outside", async ({
      page,
      request,
    }) => {
      const uuid = await fetchBookIdByTitle(request, TARGET.title);
      await openResults(page, uuid);

      await page.getByTestId("mes-close").click();
      await expect(page.getByTestId("metadata-search")).toHaveCount(0);

      await page.getByTestId("metadata-search-btn").click();
      await expect(page.getByTestId("metadata-search")).toBeVisible();
      // The scrim, not the panel — clicking the card itself must not dismiss.
      await page
        .getByTestId("metadata-search-scrim")
        .click({ position: { x: 5, y: 5 } });
      await expect(page.getByTestId("metadata-search")).toHaveCount(0);
    });

    // -------------------------------------------------------------------------
    // Results — attributed candidates in an order the providers don't decide
    // -------------------------------------------------------------------------

    test("lists a candidate per source with cover, title, authors, and origin", async ({
      page,
      request,
    }) => {
      const uuid = await fetchBookIdByTitle(request, TARGET.title);
      await openResults(page, uuid);

      await expect(page.getByTestId("mes-candidate-0-title")).toHaveText(
        OL_CANDIDATE.title,
      );
      await expect(page.getByTestId("mes-candidate-0-authors")).toHaveText(
        "Sophie Germain",
      );
      await expect(page.getByTestId("mes-candidate-0-imprint")).toHaveText(
        "1815 · Klein Mathematik · 9781234567897",
      );
      await expect(page.getByTestId("mes-candidate-0-source")).toHaveText(
        "Open Library",
      );
      await expect(
        page.getByTestId("mes-candidate-0").locator("img"),
      ).toBeVisible();

      // The same ISBN under a second source stays its own row.
      await expect(page.getByTestId("mes-candidate-1-title")).toHaveText(
        GB_CANDIDATE.title,
      );
      await expect(page.getByTestId("mes-candidate-1-source")).toHaveText(
        "Google Books",
      );
    });

    test("orders candidates by the editions themselves, not by who answered", async ({
      page,
      request,
    }) => {
      const uuid = await fetchBookIdByTitle(request, TARGET.title);
      // The stub returns Google Books first; the list must still lead with the
      // title that sorts first, so a provider being slow or newly configured
      // can't reshuffle the rows under the reader.
      await openResults(page, uuid);
      await expect(page.getByTestId("mes-candidate-0-source")).toHaveText(
        "Open Library",
      );
      await expect(page.getByTestId("mes-candidate-1-source")).toHaveText(
        "Google Books",
      );
    });

    test("says what every source contributed, including the ones that didn't", async ({
      page,
      request,
    }) => {
      const uuid = await fetchBookIdByTitle(request, TARGET.title);
      await mockProviders(page, ALL_CONFIGURED);
      await mockHydrate(page);
      await mockSearch(page, {
        editions: [OL_CANDIDATE],
        sources: [
          SOURCES[0],
          {
            provider: "google_books",
            display_name: "Google Books",
            status: { kind: "answered", count: 0 },
          },
          {
            provider: "hardcover",
            display_name: "Hardcover",
            status: { kind: "failed", message: "request timed out" },
          },
        ],
      });
      await openPicker(page, uuid);

      // Four outcomes, four distinct reads — a short list has several causes
      // and they are not interchangeable.
      await expect(page.getByTestId("mes-source-open-library")).toHaveText(
        "Open Library 1",
      );
      await expect(page.getByTestId("mes-source-google-books")).toHaveText(
        "Google Books nothing",
      );
      const failed = page.getByTestId("mes-source-hardcover");
      await expect(failed).toHaveText("Hardcover unavailable");
      // The provider's own message is available without the line becoming an
      // error log.
      await expect(failed).toHaveAttribute("title", "request timed out");
    });

    test("reports a rate-limited provider as a wait, not a failure", async ({
      page,
      request,
    }) => {
      const uuid = await fetchBookIdByTitle(request, TARGET.title);
      await mockProviders(page, ALL_CONFIGURED);
      await mockHydrate(page);
      await mockSearch(page, {
        editions: [OL_CANDIDATE],
        sources: [
          SOURCES[0],
          SOURCES[1],
          {
            provider: "hardcover",
            display_name: "Hardcover",
            // Nothing is broken: this source asked us to come back later, and
            // we did not spend a request finding that out again.
            status: { kind: "throttled", retry_after_secs: 600 },
          },
        ],
      });
      await openPicker(page, uuid);

      const throttled = page.getByTestId("mes-source-hardcover");
      await expect(throttled).toHaveText(
        "Hardcover rate limited, skipping for 10m",
      );
      await expect(throttled).toHaveAttribute(
        "title",
        /rate-limited us recently/,
      );
    });

    test("renders a candidate that names no printing without collapsing its row", async ({
      page,
      request,
    }) => {
      // A work-level search hit — Hardcover's `search` endpoint answers with
      // these — has no year, no publisher, and no edition ISBN. It is still a
      // candidate, and its row still has three lines.
      const uuid = await fetchBookIdByTitle(request, TARGET.title);
      await mockProviders(page, ALL_CONFIGURED);
      await mockHydrate(page);
      await mockSearch(page, {
        editions: [
          {
            ...OL_CANDIDATE,
            isbn13: null,
            year: null,
            publisher: null,
          },
        ],
        sources: SOURCES,
      });
      await openPicker(page, uuid);

      await expect(page.getByTestId("mes-candidate-0-title")).toHaveText(
        OL_CANDIDATE.title,
      );
      await expect(page.getByTestId("mes-candidate-0-imprint")).toHaveText(
        "\u2014",
      );
    });

    test("reports a provider this instance has no key for as not set up", async ({
      page,
      request,
    }) => {
      const uuid = await fetchBookIdByTitle(request, TARGET.title);
      await openResults(page, uuid);
      await expect(page.getByTestId("mes-source-hardcover")).toHaveText(
        "Hardcover not set up",
      );
    });

    test("surfaces a search failure and leaves the form values alone", async ({
      page,
      request,
    }) => {
      const uuid = await fetchBookIdByTitle(request, TARGET.title);
      await mockProviders(page, ALL_CONFIGURED);
      await failSearch(page);
      await openPicker(page, uuid);

      await expect(page.getByTestId("mes-error")).toBeVisible();
      await expect(page.getByTestId("mes-candidates")).toHaveCount(0);

      // The edit form never moved.
      await page.getByTestId("mes-close").click();
      await expect(page.locator("#me-title")).toHaveValue(TARGET.title);
      await expect(page.getByTestId("me-save")).toBeDisabled();
    });

    test("hides the entry point when no provider is configured", async ({
      page,
      request,
    }) => {
      const uuid = await fetchBookIdByTitle(request, TARGET.title);
      await mockProviders(page, [
        provider("open_library", "Open Library", false),
        provider("google_books", "Google Books", false),
        provider("hardcover", "Hardcover", false),
      ]);
      await gotoReady(page, `/books/${uuid}/edit`);
      await expect(page.getByTestId("metadata-search-btn")).toHaveCount(0);
    });

    // -------------------------------------------------------------------------
    // Compare — only what differs
    // -------------------------------------------------------------------------

    test("shows only the fields the source would change", async ({
      page,
      request,
    }) => {
      const uuid = await fetchBookIdByTitle(request, TARGET.title);
      await openCompare(page, uuid);

      await expect(page.getByTestId("mes-compare-source")).toHaveText(
        "Google Books",
      );
      await expect(page.getByTestId("mes-row-title-source")).toHaveText(
        GB_CANDIDATE.title,
      );
      await expect(page.getByTestId("mes-row-publisher-source")).toHaveText(
        "Editions Gauss",
      );
      // Absent, not an em-dash row: this source has no series, so it can't
      // change one, and a row that can't be applied is only noise.
      await expect(page.getByTestId("mes-row-series")).toHaveCount(0);
      await expect(page.getByTestId("mes-row-genres")).toHaveCount(0);

      // Nothing staged by looking.
      await expect(page.getByTestId("me-save")).toBeDisabled();
    });

    test("reveals the untouched fields on request and hides them again", async ({
      page,
      request,
    }) => {
      const uuid = await fetchBookIdByTitle(request, TARGET.title);
      await openCompare(page, uuid);
      await expect(page.getByTestId("mes-row-series")).toHaveCount(0);

      await page.getByTestId("mes-show-all").click();
      // Now visible with an em dash and a dead arrow: the source has nothing to
      // offer, and a provider not knowing a field must never blank out a value
      // you already have.
      await expect(page.getByTestId("mes-row-series-source")).toHaveText("—");
      await expect(page.getByTestId("mes-row-series-apply")).toBeDisabled();
      await expect(page.getByTestId("mes-row-series-current")).toHaveText(
        TARGET.series!,
      );

      await page.getByTestId("mes-show-all").click();
      await expect(page.getByTestId("mes-row-series")).toHaveCount(0);
    });

    test("copies one field into the form and keeps its row in place", async ({
      page,
      request,
    }) => {
      const uuid = await fetchBookIdByTitle(request, TARGET.title);
      await openCompare(page, uuid);
      await expect(page.getByTestId("mes-row-publisher-current")).toHaveText(
        TARGET.publisher!,
      );

      await page.getByTestId("mes-row-publisher-apply").click();

      await expect(page.locator("#me-publisher")).toHaveValue("Editions Gauss");
      await expect(page.locator("#me-title")).toHaveValue(TARGET.title);
      // The row stays and its control becomes the way back — applying must
      // not make a row vanish out from under the cursor just because it
      // stopped differing.
      await expect(page.getByTestId("mes-row-publisher")).toHaveAttribute(
        "data-staged",
        "true",
      );
      await expect(page.getByTestId("mes-row-publisher-apply")).toHaveCount(0);
      await expect(page.getByTestId("mes-row-publisher-undo")).toBeVisible();
      await expect(page.getByTestId("mes-row-publisher-current")).toHaveText(
        "Editions Gauss",
      );

      // Staged only — the save bar counts it, and no write went out.
      await expect(page.getByTestId("me-save")).toBeEnabled();
      await expect(page.getByText("1 field edited")).toBeVisible();

      await page.getByTestId("mes-close").click();
      await page.getByTestId("me-discard").click();
    });

    test("undoes a staged field back to what the book had", async ({
      page,
      request,
    }) => {
      const uuid = await fetchBookIdByTitle(request, TARGET.title);
      await openCompare(page, uuid);
      await page.getByTestId("mes-row-publisher-apply").click();
      await expect(page.locator("#me-publisher")).toHaveValue("Editions Gauss");

      await page.getByTestId("mes-row-publisher-undo").click();

      // Back to the book's own value, and the row offers to take it again —
      // the control is what says which state the row is in.
      await expect(page.locator("#me-publisher")).toHaveValue(
        TARGET.publisher!,
      );
      await expect(page.getByTestId("mes-row-publisher")).toHaveAttribute(
        "data-staged",
        "false",
      );
      await expect(page.getByTestId("mes-row-publisher-apply")).toBeVisible();
      // And the save bar agrees there is nothing to save.
      await expect(page.getByTestId("me-save")).toBeDisabled();
    });

    test("offers every field the retired Hardcover panel could apply", async ({
      page,
      request,
    }) => {
      // AC2 of #1665: retiring the one-provider panel must not lose a field.
      // It could apply title, authors, description, series, book # and
      // ISBN-13, so all six have to be reachable through the fan-out before
      // it goes — and reachable *from Hardcover*, since that is the provider
      // the panel spoke to and the only one that publishes a book number.
      const uuid = await fetchBookIdByTitle(request, TARGET.title);
      await mockProviders(page, HC_CONFIGURED);
      await mockSearch(page, HC_FOUND);
      await mockHydrate(page);
      await openPicker(page, uuid);

      // Hardcover is a source in its own right, not a "not set up" row.
      await expect(page.getByTestId("mes-source-hardcover")).toHaveText(
        /Hardcover/,
      );
      await page.getByTestId("mes-candidate-0").click();
      await expect(page.getByTestId("mes-compare")).toBeVisible();

      for (const slug of [
        "title",
        "author-s-",
        "description",
        "series",
        "book--",
        "isbn-13",
      ]) {
        await expect(page.getByTestId(`mes-row-${slug}`)).toBeVisible();
        await expect(page.getByTestId(`mes-row-${slug}-apply`)).toBeEnabled();
      }

      // Book # is the arm the retirement had to add: the panel could set one
      // and the picker had no field for it.
      await page.getByTestId("mes-row-book---apply").click();
      await expect(page.locator("#me-book--")).toHaveValue("7");

      await page.getByTestId("mes-close").click();
      await page.getByTestId("me-discard").click();
    });

    test("takes every change at once and skips what the source lacks", async ({
      page,
      request,
    }) => {
      const uuid = await fetchBookIdByTitle(request, TARGET.title);
      await openCompare(page, uuid);

      await page.getByTestId("mes-take-all").click();

      await expect(page.locator("#me-title")).toHaveValue(GB_CANDIDATE.title);
      await expect(page.locator("#me-publisher")).toHaveValue("Editions Gauss");
      await expect(page.locator("#me-published")).toHaveValue("1816");
      await expect(page.locator("#me-isbn-13")).toHaveValue("9781234567897");
      await expect(page.locator("#me-print-pages")).toHaveValue("220");
      await expect(page.locator("#me-description")).toHaveValue(
        "A Google Books description.",
      );
      await expect(
        page.locator(".me-chip-item").getByText("Emmy Noether"),
      ).toBeVisible();
      // Google Books offered no series, so the book's own value survives —
      // take-all skips what the source lacks rather than blanking it.
      await expect(page.locator("#me-series")).toHaveValue(TARGET.series!);
      // Nothing is left to take.
      await expect(page.getByTestId("mes-take-all")).toBeDisabled();

      await page.getByTestId("mes-close").click();
      await page.getByTestId("me-discard").click();
    });

    test("keeps a staged apply across a trip back to the results", async ({
      page,
      request,
    }) => {
      const uuid = await fetchBookIdByTitle(request, TARGET.title);
      await openCompare(page, uuid);
      await page.getByTestId("mes-row-publisher-apply").click();

      await page.getByTestId("mes-compare-back").click();
      await expect(page.getByTestId("mes-candidates")).toBeVisible();
      await expect(page.getByTestId("mes-compare")).toHaveCount(0);

      // Still staged: the compare screen writes into the page's form signals,
      // which outlive the overlay.
      await expect(page.locator("#me-publisher")).toHaveValue("Editions Gauss");
      await expect(page.getByText("1 field edited")).toBeVisible();

      // And closing the overlay doesn't discard them either.
      await page.getByTestId("mes-close").click();
      await expect(page.locator("#me-publisher")).toHaveValue("Editions Gauss");

      await page.getByTestId("me-discard").click();
    });

    test("discards a staged-but-unsaved apply on navigating away", async ({
      page,
      request,
    }) => {
      const uuid = await fetchBookIdByTitle(request, TARGET.title);
      await openCompare(page, uuid);
      await page.getByTestId("mes-row-publisher-apply").click();
      await page.getByTestId("mes-done").click();
      await expect(page.getByTestId("metadata-search")).toHaveCount(0);

      await page.getByTestId("me-discard").click();
      await expect(page).toHaveURL(new RegExp(`/books/${uuid}$`));

      await gotoReady(page, `/books/${uuid}/edit`);
      await expect(page.locator("#me-publisher")).toHaveValue(
        TARGET.publisher!,
      );
      await expect(page.getByTestId("me-save")).toBeDisabled();
    });

    test("saving after an apply persists exactly the applied values", async ({
      page,
      request,
    }) => {
      const uuid = await fetchBookIdByTitle(request, TARGET.title);
      await openCompare(page, uuid);
      await page.getByTestId("mes-row-publisher-apply").click();
      await page.getByTestId("mes-done").click();

      // Through the page's existing save path — the compare screen never talks
      // to the overrides endpoint itself.
      await expectMutation(
        page,
        {
          method: "POST",
          url: /\/api\/rpc\/ebook\/overrides$/,
          expectedBody: { uuid, overrides: { publisher: "Editions Gauss" } },
          expectedStatus: 200,
        },
        async () => page.getByTestId("me-save").click(),
      );
      await expect(page).toHaveURL(new RegExp(`/books/${uuid}$`));

      // Exactly the applied field: the title the source also offered was never
      // taken, so it must not have been written.
      await gotoReady(page, `/books/${uuid}/edit`);
      await expect(page.locator("#me-publisher")).toHaveValue("Editions Gauss");
      await expect(page.locator("#me-title")).toHaveValue(TARGET.title);

      // Put the fixture back for every other spec and run.
      await expectMutation(
        page,
        {
          method: "POST",
          url: /\/api\/rpc\/ebook\/overrides\/delete$/,
          expectedStatus: 200,
        },
        async () => page.getByTestId("revert-overrides").click(),
      );
    });

    // -------------------------------------------------------------------------
    // Nothing is actionable while the record is still being replaced
    // -------------------------------------------------------------------------

    test("holds the apply controls inert until the detail re-fetch lands", async ({
      page,
      request,
    }) => {
      const uuid = await fetchBookIdByTitle(request, TARGET.title);
      await mockProviders(page, ALL_CONFIGURED);
      await mockSearch(page, FOUND);

      // A hydrate that never answers until we let it: the compare screen is
      // painted against the thin search hit for the whole of this window.
      let releaseHydrate: (() => void) | undefined;
      const hydrateHeld = new Promise<void>((resolve) => {
        releaseHydrate = resolve;
      });
      await page.route(HYDRATE_URL, async (route) => {
        if (route.request().method() !== "POST") return route.continue();
        await hydrateHeld;
        return route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({
            ...OL_CANDIDATE,
            description: "Only the detail record has this.",
          }),
        });
      });

      await openPicker(page, uuid);
      await page.getByTestId("mes-candidate-0").click();

      // In flight: a placeholder in the table's shape, and no field rows at
      // all. The search hit this screen opens with is a partial answer — the
      // Open Library hit has no description and its record does — so showing
      // it would mean rows appearing and rearranging a moment later, and
      // "take all" would mean a different set of fields depending on how fast
      // the network answered.
      await expect(page.getByTestId("mes-hydrating")).toBeVisible();
      await expect(page.getByTestId("mes-compare-skeleton")).toBeVisible();
      await expect(page.getByTestId("mes-compare-fields")).toHaveCount(0);
      await expect(page.getByTestId("mes-take-all")).toBeDisabled();

      releaseHydrate?.();

      // Settled: the description arrived and every control is live again.
      await expect(page.getByTestId("mes-row-description-source")).toHaveText(
        "Only the detail record has this.",
      );
      await expect(page.getByTestId("mes-take-all")).toBeEnabled();

      await page.getByTestId("mes-take-all").click();
      await expect(page.locator("#me-description")).toHaveValue(
        "Only the detail record has this.",
      );

      await page.getByTestId("mes-close").click();
      await page.getByTestId("me-discard").click();
    });

    // -------------------------------------------------------------------------
    // The cover row — the one field that writes immediately
    // -------------------------------------------------------------------------

    test("offers the source's cover with an arrow labelled as an immediate change", async ({
      page,
      request,
    }) => {
      const uuid = await fetchBookIdByTitle(request, TARGET.title);
      await openCompare(page, uuid);

      await expect(page.getByTestId("mes-row-cover")).toBeVisible();
      await expect(
        page.getByTestId("mes-row-cover-source").locator("img"),
      ).toBeVisible();
      // The row says so in words, not only in behaviour.
      await expect(page.getByTestId("mes-row-cover-note")).toHaveText(
        "The cover applies immediately · it isn’t staged with the fields",
      );
      await expect(page.getByTestId("mes-row-cover-apply")).toHaveAttribute(
        "aria-label",
        /saves immediately/,
      );
    });

    test("hides the cover row when the source has no cover to offer", async ({
      page,
      request,
    }) => {
      const uuid = await fetchBookIdByTitle(request, TARGET.title);
      await mockProviders(page, ALL_CONFIGURED);
      await mockHydrate(page);
      await mockSearch(page, {
        ...FOUND,
        editions: [{ ...OL_CANDIDATE, cover_url: null }],
      });
      await openPicker(page, uuid);
      await page.getByTestId("mes-candidate-0").click();

      await expect(page.getByTestId("mes-row-cover")).toHaveCount(0);
      // Held to the same rule as every field row, so "show all" brings it back.
      await page.getByTestId("mes-show-all").click();
      await expect(page.getByTestId("mes-row-cover-apply")).toBeDisabled();
    });

    test("applying the source's cover writes immediately and reports it", async ({
      page,
      request,
    }) => {
      const uuid = await fetchBookIdByTitle(request, TARGET.title);
      await page.route(COVER_FROM_URL, async (route) => {
        if (route.request().method() !== "POST") return route.continue();
        // Echo the book back with **both** flags the real handler sets: a cover
        // override is an override row, so `has_override` flips too — and it is
        // what the sidebar's "Override active" card reads.
        const resp = await request.get(`/api/ebooks/${uuid}`);
        const book = (await resp.json()) as Record<string, unknown>;
        return route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({
            ...book,
            has_cover_override: true,
            has_override: true,
          }),
        });
      });
      await openCompare(page, uuid);

      await expectMutation(
        page,
        {
          method: "POST",
          url: new RegExp(`/api/ebooks/${uuid}/cover/from-url$`),
          expectedBody: { url: PIXEL },
          expectedStatus: 200,
        },
        async () => page.getByTestId("mes-row-cover-apply").click(),
      );

      await expect(page.getByTestId("mes-row-cover-note")).toHaveText(
        "Cover updated.",
      );
      // Immediate, not staged: the save bar never noticed.
      await expect(page.getByTestId("me-save")).toBeDisabled();

      // And the sidebar follows the write without a reload — both its cover
      // controls and its "Override active" card, which live in two different
      // components behind the overlay.
      await page.getByTestId("mes-close").click();
      await expect(page.getByTestId("cover-remove-override")).toBeVisible();
      await expect(page.getByTestId("cover-hint")).toHaveText("custom upload");
      await expect(page.getByTestId("revert-overrides")).toBeVisible();
    });

    test("surfaces a refused cover without changing anything", async ({
      page,
      request,
    }) => {
      const uuid = await fetchBookIdByTitle(request, TARGET.title);
      await page.route(COVER_FROM_URL, (route) =>
        route.request().method() === "POST"
          ? route.fulfill({
              status: 400,
              contentType: "text/plain",
              body: "host is not an allowed source: evil.example",
            })
          : route.continue(),
      );
      await openCompare(page, uuid);

      await expectMutation(
        page,
        {
          method: "POST",
          url: new RegExp(`/api/ebooks/${uuid}/cover/from-url$`),
          expectedStatus: 400,
        },
        async () => page.getByTestId("mes-row-cover-apply").click(),
      );

      await expect(page.getByTestId("mes-row-cover-note")).toContainText(
        "Couldn't apply that cover",
      );
      await page.getByTestId("mes-close").click();
      await expect(page.getByTestId("cover-remove-override")).toHaveCount(0);
      await expect(page.getByTestId("me-save")).toBeDisabled();
    });
  });

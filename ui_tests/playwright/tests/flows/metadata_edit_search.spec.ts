import type { Page } from "@playwright/test";
import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { expect, test } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import { fetchBookIdByTitle } from "../utils/ebooks";
import { gotoReady } from "../utils/nav";
import { fixturesDir, seedLibrary } from "../utils/seed";

// Edition picker on the metadata-edit page (#1661) and the side-by-side
// compare view a selected candidate opens into (#1662): a search that fans
// out to every configured provider and lists the editions they found, with a
// per-source status area so a short list can't be mistaken for a complete
// one. Each compare row carries an arrow that stages that field into the
// edit form; the form's own Save is still the only thing that writes.
//
// Every provider response is stubbed via `page.route`. The suite must never
// depend on a live provider or a configured key — the same reasoning
// `metadata_edit_hardcover_fetch.spec.ts` gives, and doubly so here, since
// the fan-out would otherwise spend three real API calls per test.
//
// `mathematica-minor-2` ("Minor Lemmas II", Sophie Germain) is read by no
// other spec — see the fixture-isolation note in
// `.claude/rules/04-playwright.md`. Nothing here saves, so the book's
// override state is never touched either way.

test.beforeAll(async ({ request }) => {
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
});

const TARGET = FIXTURE_BOOKS.find((b) => b.slug === "mathematica-minor-2")!;

const PROVIDERS_URL = "**/api/rpc/metadata/providers";
const SEARCH_URL = "**/api/rpc/metadata/editions/search";
const HYDRATE_URL = "**/api/rpc/metadata/editions/hydrate";

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

// One candidate per source, deliberately sharing an ISBN across two of them:
// the picker's job is to keep two sources' takes on one edition apart.
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
  first_publish_year: 1815,
  genres: ["Mathematics"],
};

const GB_CANDIDATE = {
  source: "google_books",
  provider_ref: "gb-volume-7",
  isbn13: "9781234567897",
  title: "Minor Lemmas II (Google Books)",
  authors: ["Sophie Germain", "Emmy Noether"],
  year: "1816",
  pages: 220,
  publisher: "Editions Gauss",
  description: "A Google Books description.",
  cover_url: PIXEL,
  series: null,
  first_publish_year: null,
  genres: [],
};

const FOUND = {
  editions: [OL_CANDIDATE, GB_CANDIDATE],
  sources: [
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
  ],
};

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

/** Hydrate answers `null` unless a test says otherwise — the picker keeps
 * the candidate it already has, which is the behaviour most tests want. */
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

/** Open the picker on `uuid`'s edit page with the given provider catalog. */
async function openPicker(
  page: Page,
  uuid: string,
  catalog = ALL_CONFIGURED,
): Promise<void> {
  await mockProviders(page, catalog);
  await mockHydrate(page);
  await gotoReady(page, `/books/${uuid}/edit`);
  await page.getByTestId("metadata-search-btn").click();
}

/** Run the search and await its response, so DOM assertions don't race the
 * network (per `.claude/rules/04-playwright.md`). */
async function runSearch(page: Page): Promise<void> {
  const searched = page.waitForResponse(
    (r) =>
      r.url().includes("/api/rpc/metadata/editions/search") &&
      r.request().method() === "POST",
  );
  await page.getByTestId("mes-search").click();
  await searched;
}

// One test below saves an override on the fixture book and reverts it; every
// other test looks the book up by its original title. `describe.serial` pins
// them to one worker (the same reasoning `metadata_edit.spec.ts` gives) so a
// save can never be mid-flight while another test reads the book.
test.describe
  .serial("metadata edit edition picker", () => {
    // ---------------------------------------------------------------------------
    // AC1 — layout: the entry point exists and the panel opens prefilled
    // ---------------------------------------------------------------------------

    test("renders the edition-search entry point and opens prefilled from the book", async ({
      page,
      request,
    }) => {
      const uuid = await fetchBookIdByTitle(request, TARGET.title);
      await mockProviders(page, ALL_CONFIGURED);
      await gotoReady(page, `/books/${uuid}/edit`);

      const open = page.getByTestId("metadata-search-btn");
      await expect(open).toBeVisible();
      await expect(open).toHaveAttribute("aria-expanded", "false");

      await open.click();
      await expect(open).toHaveAttribute("aria-expanded", "true");
      await expect(page.getByTestId("mes-query")).toHaveValue(
        `${TARGET.title} ${TARGET.authors[0]}`,
      );
      await expect(page.getByTestId("mes-search")).toBeEnabled();
    });

    // ---------------------------------------------------------------------------
    // AC2 — candidates from multiple sources, each carrying its own attribution
    // ---------------------------------------------------------------------------

    test("lists a candidate per source with cover, title, authors, imprint, and origin", async ({
      page,
      request,
    }) => {
      const uuid = await fetchBookIdByTitle(request, TARGET.title);
      await openPicker(page, uuid);
      await mockSearch(page, FOUND);
      await runSearch(page);

      await expect(page.getByTestId("mes-candidates")).toBeVisible();
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
      await expect(page.getByTestId("mes-candidate-1-authors")).toHaveText(
        "Sophie Germain, Emmy Noether",
      );
      await expect(page.getByTestId("mes-candidate-1-source")).toHaveText(
        "Google Books",
      );
    });

    // ---------------------------------------------------------------------------
    // AC3 — the four per-source outcomes read differently
    // ---------------------------------------------------------------------------

    test("distinguishes answered, empty, not-configured, and failed sources", async ({
      page,
      request,
    }) => {
      const uuid = await fetchBookIdByTitle(request, TARGET.title);
      await openPicker(page, uuid);
      await mockSearch(page, {
        editions: [OL_CANDIDATE],
        sources: [
          {
            provider: "open_library",
            display_name: "Open Library",
            status: { kind: "answered", count: 1 },
          },
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
      await runSearch(page);

      await expect(page.getByTestId("mes-source-open-library")).toHaveText(
        "Open Library—1 edition",
      );
      await expect(page.getByTestId("mes-source-google-books")).toHaveText(
        "Google Books—no matches",
      );
      const failed = page.getByTestId("mes-source-hardcover");
      await expect(failed).toHaveText("Hardcover—unavailable");
      // The provider's own message is available without turning the list into an
      // error log.
      await expect(failed).toHaveAttribute("title", "request timed out");
    });

    test("reports a provider this instance has no key for as not configured", async ({
      page,
      request,
    }) => {
      const uuid = await fetchBookIdByTitle(request, TARGET.title);
      await openPicker(page, uuid);
      await mockSearch(page, FOUND);
      await runSearch(page);

      // Distinct from both "no matches" and "unavailable" above.
      await expect(page.getByTestId("mes-source-hardcover")).toHaveText(
        "Hardcover—not configured",
      );
    });

    // ---------------------------------------------------------------------------
    // AC4 — error path: the search fails, the form is untouched
    // ---------------------------------------------------------------------------

    test("surfaces a search failure and leaves the form values alone", async ({
      page,
      request,
    }) => {
      const uuid = await fetchBookIdByTitle(request, TARGET.title);
      await openPicker(page, uuid);
      await failSearch(page);
      await runSearch(page);

      await expect(page.getByTestId("mes-error")).toBeVisible();
      await expect(page.getByTestId("mes-candidates")).toHaveCount(0);
      // The edit form never moved: same title, nothing staged, save bar clean.
      await expect(page.locator("#me-title")).toHaveValue(TARGET.title);
      await expect(page.getByTestId("me-save")).toBeDisabled();
    });

    // ---------------------------------------------------------------------------
    // AC5 — selecting a candidate opens the compare view for that edition
    // ---------------------------------------------------------------------------

    test("opens the compare view for the selected candidate and returns to the list", async ({
      page,
      request,
    }) => {
      const uuid = await fetchBookIdByTitle(request, TARGET.title);
      await openPicker(page, uuid);
      await mockSearch(page, FOUND);
      await runSearch(page);

      await page.getByTestId("mes-candidate-1").click();

      const compare = page.getByTestId("mes-compare");
      await expect(compare).toBeVisible();
      await expect(page.getByTestId("mes-compare-source")).toHaveText(
        "Google Books",
      );
      await expect(page.getByTestId("mes-row-title-source")).toHaveText(
        GB_CANDIDATE.title,
      );
      await expect(page.getByTestId("mes-row-publisher-source")).toHaveText(
        "Editions Gauss",
      );
      // A field this source has no value for renders as an em dash rather than
      // vanishing — the reader can tell "Google Books has no series" from "this
      // screen forgot to show one".
      await expect(page.getByTestId("mes-row-series-source")).toHaveText("—");
      // Selecting stages nothing: the form and its save bar are untouched.
      await expect(page.locator("#me-title")).toHaveValue(TARGET.title);
      await expect(page.getByTestId("me-save")).toBeDisabled();

      await page.getByTestId("mes-compare-back").click();
      await expect(page.getByTestId("mes-candidates")).toBeVisible();
      await expect(compare).toHaveCount(0);
    });

    test("fills a field the search hit lacked from the selected edition's own record", async ({
      page,
      request,
    }) => {
      const uuid = await fetchBookIdByTitle(request, TARGET.title);
      await mockProviders(page, ALL_CONFIGURED);
      // Open Library's search docs carry no description; its record does. The
      // merge must add that without taking the row's publisher away.
      await mockHydrate(page, {
        ...OL_CANDIDATE,
        description: "The record's own description.",
        publisher: null,
      });
      await gotoReady(page, `/books/${uuid}/edit`);
      await page.getByTestId("metadata-search-btn").click();
      await mockSearch(page, FOUND);
      await runSearch(page);

      const hydrated = page.waitForResponse(
        (r) =>
          r.url().includes("/api/rpc/metadata/editions/hydrate") &&
          r.request().method() === "POST",
      );
      await page.getByTestId("mes-candidate-0").click();
      await hydrated;

      await expect(page.getByTestId("mes-row-description-source")).toHaveText(
        "The record's own description.",
      );
      await expect(page.getByTestId("mes-row-publisher-source")).toHaveText(
        "Klein Mathematik",
      );
    });

    // ---------------------------------------------------------------------------
    // AC6 — no usable provider, no entry point
    // ---------------------------------------------------------------------------

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

    // ---------------------------------------------------------------------------
    // #1662 AC1/AC2 — one row, one arrow, staged into the form and nowhere else
    // ---------------------------------------------------------------------------

    /** Open the compare view for the Google Books candidate, which answers for a
     * description and a publisher but has no series. */
    async function openCompare(page: Page, uuid: string): Promise<void> {
      await openPicker(page, uuid);
      await mockSearch(page, FOUND);
      await runSearch(page);
      await page.getByTestId("mes-candidate-1").click();
      await expect(page.getByTestId("mes-compare")).toBeVisible();
    }

    test("copies one field into the form and leaves every other field alone", async ({
      page,
      request,
    }) => {
      const uuid = await fetchBookIdByTitle(request, TARGET.title);
      await openCompare(page, uuid);

      // Yours-vs-theirs, before.
      await expect(page.getByTestId("mes-row-publisher-current")).toHaveText(
        TARGET.publisher!,
      );
      await expect(page.getByTestId("mes-row-publisher-source")).toHaveText(
        "Editions Gauss",
      );

      await page.getByTestId("mes-row-publisher-apply").click();

      // The form moved; nothing else did.
      await expect(page.locator("#me-publisher")).toHaveValue("Editions Gauss");
      await expect(page.locator("#me-title")).toHaveValue(TARGET.title);
      await expect(page.locator("#me-description")).toHaveValue("");
      // And the row now reads as taken, without the control going away.
      await expect(page.getByTestId("mes-row-publisher-current")).toHaveText(
        "Editions Gauss",
      );
      await expect(page.getByTestId("mes-row-publisher")).toHaveAttribute(
        "data-applied",
        "true",
      );
      await expect(page.getByTestId("mes-row-publisher-apply")).toBeEnabled();

      // Staged only — the save bar counts it, and no write went out.
      await expect(page.getByTestId("me-save")).toBeEnabled();
      await expect(page.getByText("1 field edited")).toBeVisible();
      await expect(page).toHaveURL(new RegExp(`/books/${uuid}/edit$`));

      await page.getByTestId("me-discard").click();
    });

    // ---------------------------------------------------------------------------
    // #1662 AC3 — take everything, skipping what the source lacks
    // ---------------------------------------------------------------------------

    test("take-all applies every field the source has and skips the ones it lacks", async ({
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
      // Google Books offered no series for this edition, so the book's own value
      // survives — take-all skips what the source lacks rather than blanking it
      // "successfully", which is the most damaging thing this screen could do.
      await expect(page.locator("#me-series")).toHaveValue(TARGET.series!);

      await page.getByTestId("me-discard").click();
    });

    // ---------------------------------------------------------------------------
    // #1662 AC4 — a field the source lacks cannot be applied
    // ---------------------------------------------------------------------------

    test("cannot apply a field the source has no value for", async ({
      page,
      request,
    }) => {
      const uuid = await fetchBookIdByTitle(request, TARGET.title);
      await openCompare(page, uuid);

      // The em dash is the tell, and the control is dead: the most damaging thing
      // this screen could do is blank a value you already have — and this book
      // does have a series, which this source doesn't know.
      await expect(page.getByTestId("mes-row-series-current")).toHaveText(
        TARGET.series!,
      );
      await expect(page.getByTestId("mes-row-series-source")).toHaveText("—");
      await expect(page.getByTestId("mes-row-series-apply")).toBeDisabled();

      // Genres too — this source returned an empty list, which is not a value.
      await expect(page.getByTestId("mes-row-genres-source")).toHaveText("—");
      await expect(page.getByTestId("mes-row-genres-apply")).toBeDisabled();

      await expect(page.getByTestId("me-save")).toBeDisabled();
    });

    // ---------------------------------------------------------------------------
    // #1662 — staged edits survive the trip back to the results
    // ---------------------------------------------------------------------------

    test("keeps a staged apply when returning to the candidate list", async ({
      page,
      request,
    }) => {
      const uuid = await fetchBookIdByTitle(request, TARGET.title);
      await openCompare(page, uuid);
      await page.getByTestId("mes-row-publisher-apply").click();
      await expect(page.locator("#me-publisher")).toHaveValue("Editions Gauss");

      await page.getByTestId("mes-compare-back").click();
      await expect(page.getByTestId("mes-candidates")).toBeVisible();

      // Still staged: the compare view writes into the page's form signals, which
      // outlive the panel.
      await expect(page.locator("#me-publisher")).toHaveValue("Editions Gauss");
      await expect(page.getByText("1 field edited")).toBeVisible();

      await page.getByTestId("me-discard").click();
    });

    test("discards a staged-but-unsaved apply on navigating away", async ({
      page,
      request,
    }) => {
      const uuid = await fetchBookIdByTitle(request, TARGET.title);
      await openCompare(page, uuid);
      await page.getByTestId("mes-row-publisher-apply").click();
      await expect(page.locator("#me-publisher")).toHaveValue("Editions Gauss");

      await page.getByTestId("me-discard").click();
      await expect(page).toHaveURL(new RegExp(`/books/${uuid}$`));

      await gotoReady(page, `/books/${uuid}/edit`);
      await expect(page.locator("#me-publisher")).toHaveValue(
        TARGET.publisher!,
      );
      await expect(page.getByTestId("me-save")).toBeDisabled();
    });

    // ---------------------------------------------------------------------------
    // #1662 AC5 — saving after an apply persists exactly the applied values
    // ---------------------------------------------------------------------------

    test("saving after an apply persists exactly the applied values", async ({
      page,
      request,
    }) => {
      const uuid = await fetchBookIdByTitle(request, TARGET.title);
      await openCompare(page, uuid);
      await page.getByTestId("mes-row-publisher-apply").click();

      // Through the page's existing save path — the compare view never talks to
      // the overrides endpoint itself.
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
      await expect(page).toHaveURL(new RegExp(`/books/${uuid}$`));
    });
  }); // test.describe.serial

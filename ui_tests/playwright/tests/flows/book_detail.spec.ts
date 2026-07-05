import { expect, test } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import { AUDIOBOOK_BOOKS, AUDIOBOOK_BOOK_COUNT } from "../fixtures/audiobooks";
import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { fetchBookUuidByTitle, getRow } from "../utils/ebooks";
import { gotoReady } from "../utils/nav";
import {
  audiobookFixturesDir,
  fixturesDir,
  seedAudiobookLibrary,
  seedLibrary,
} from "../utils/seed";

// Force serial mode for this file. The audiobook re-seed inside the
// `describe("audiobook-only seed", …)` block below mutates shared server
// state (re-points the library at the audiobook fixtures), so it must
// not race the ebook-seeded tests above — `playwright.config.ts` runs
// with `fullyParallel: true`, which otherwise interleaves them.
test.describe.configure({ mode: "serial" });

// Re-seed in this spec's beforeAll so the running server is indexed against
// the committed EPUB fixtures before any assertion runs — independent of
// whatever other specs in the same worker did before us. The audiobook
// library is seeded in a separate beforeAll (sharded by test.describe) so the
// listen-CTA test gets MP3/M4B-only books to land on.
test.beforeAll(async ({ request }) => {
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
});

// A fixture with predictable, distinctive metadata to drive the layout and
// breadcrumb tests. `alpha` is a single-author standalone ebook (Ada
// Lovelace) so the breadcrumb is Home > Ada Lovelace > Alpha — and we can
// reuse it for the "From the same hand" empty-state test because it's the
// only book by that author in the seed.
const TARGET = FIXTURE_BOOKS.find((b) => b.slug === "alpha")!;

// Niklaus Wirth has four books in FIXTURE_BOOKS (the Code Quartet) — three
// where he's the sole/lead author and one (Quartet III) where he's the lead
// of a multi-author group. `data::get_author` returns every book under the
// primary author's id, so visiting any Wirth book should surface the other
// three under "From the same hand".
const WIRTH_LEAD = FIXTURE_BOOKS.find((b) => b.slug === "code-quartet-1")!;
const WIRTH_OTHERS = FIXTURE_BOOKS.filter(
  (b) => b.authors[0] === "Niklaus Wirth" && b.slug !== WIRTH_LEAD.slug,
);

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

test("renders the book detail layout", async ({ page, request }) => {
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${uuid}`);

  // Hero — title heading is the H1.
  await expect(page.getByRole("heading", { level: 1, name: TARGET.title })).toBeVisible();

  // Hero — author appears in the dedicated authors line as a router link
  // (creators[0] has an id in the seeded DB). Scoped to `book-authors`
  // because the breadcrumb also renders the same name.
  const primaryAuthor = TARGET.authors[0]!;
  const authorsLine = page.getByTestId("book-authors");
  await expect(authorsLine).toContainText(primaryAuthor);
  await expect(
    authorsLine.getByRole("link", { name: primaryAuthor }),
  ).toBeVisible();

  // Hero — cover renders inside the cover column. There's also one cover
  // per "From the same hand" tile when populated, so scope to `.first()`
  // — it's the hero cover (alpha's empty-state case has no tile covers).
  await expect(page.getByTestId("cover").first()).toBeVisible();

  // Hero — primary CTA: alpha is EPUB-only, so the "Start reading" link
  // must be present and href into the /read/:uuid reader route.
  const startReading = page.getByTestId("start-reading");
  await expect(startReading).toBeVisible();
  await expect(startReading).toHaveAttribute("href", new RegExp(`/read/${uuid}$`));

  // Hero — breadcrumb landmark with Home link.
  const crumb = page.getByRole("navigation", { name: "breadcrumb" });
  await expect(crumb).toBeVisible();
  await expect(crumb.getByRole("link", { name: "Home" })).toBeVisible();

  // Body — "From the same hand" section heading is always rendered.
  await expect(page.getByRole("heading", { name: "From the same hand" })).toBeVisible();

  // Footer — Back to library link.
  await expect(page.getByRole("link", { name: "Back to library" })).toBeVisible();
});

// ---------------------------------------------------------------------------
// Action — start reading (ebook)
// ---------------------------------------------------------------------------

test("Start reading navigates to /read/:uuid for ebook-format books", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${uuid}`);

  // The hero CTA is a router Link — clicking it triggers SPA navigation.
  // The reader page mounts the full-screen reader chrome, so we just
  // assert the URL flips to /read/:uuid; the reader spec covers its
  // contents in detail.
  await page.getByTestId("start-reading").click();
  await expect(page).toHaveURL(new RegExp(`/read/${uuid}$`));
});

// ---------------------------------------------------------------------------
// Action — "From the same hand" populated row
// ---------------------------------------------------------------------------

test("From the same hand row renders other books by the same author", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, WIRTH_LEAD.title);
  await gotoReady(page, `/books/${uuid}`);

  // The author-fetch is awaited after the initial render, so poll for the
  // populated row rather than asserting visibility synchronously.
  const row = page.getByTestId("from-same-hand");
  await expect(row).toBeVisible();
  // Empty-state marker must NOT be present once the row is populated.
  await expect(page.getByTestId("from-same-hand-empty")).toHaveCount(0);

  // The row should hold exactly the other Wirth books — never the current
  // book (the page filters by `unique_identifier` to drop the self-tile).
  const tiles = row.getByTestId("from-same-hand-tile");
  await expect(tiles).toHaveCount(WIRTH_OTHERS.length);

  // The current book's cover-of-{title} alt must not appear under any of
  // the row's tiles — verifies the self-exclusion behaviour from
  // `pages/book_detail.rs::BookDetailPage`.
  await expect(
    row.getByRole("img", { name: `Cover of ${WIRTH_LEAD.title}` }),
  ).toHaveCount(0);

  // Clicking any sibling tile must navigate to its /books/:uuid page —
  // tiles are router Links that swap the route param in place. The Atrium
  // stack overlaps the tiles behind the author-lead card and only spreads
  // them apart on row hover, so hover first to settle the layout before
  // clicking — otherwise the tile's centre sits under the lead card and the
  // click is intercepted (matches the "Hover the row to spread" affordance).
  await row.hover();
  await tiles.first().click();
  await expect(page).toHaveURL(/\/books\/[0-9a-fA-F-]{36}$/);
  await expect(page).not.toHaveURL(new RegExp(`/books/${uuid}$`));
});

// ---------------------------------------------------------------------------
// Action — "From the same hand" empty state
// ---------------------------------------------------------------------------

test("From the same hand shows empty state for single-book authors", async ({
  page,
  request,
}) => {
  // Ada Lovelace is the only author of `alpha` in the fixture set — so the
  // author-fetch returns one book, filtering it out yields an empty list,
  // and the page renders `from-same-hand-empty` instead of the row.
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${uuid}`);

  // Wait for the heading to confirm the section rendered.
  await expect(page.getByRole("heading", { name: "From the same hand" })).toBeVisible();

  // Empty-state card must be visible; the populated row must be absent.
  await expect(page.getByTestId("from-same-hand-empty")).toBeVisible();
  await expect(page.getByTestId("from-same-hand")).toHaveCount(0);
});

// ---------------------------------------------------------------------------
// Pre-existing coverage — kept so the spec remains a single source of truth
// for the book detail page. These post-date the original landing-row entry
// from #297; the new tests above were added for #385.
// ---------------------------------------------------------------------------

test("navigates from a landing row to the detail page and back", async ({ page }) => {
  await gotoReady(page, "/");

  // Click the row's cover cell to follow the SPA navigation. We target the
  // cover specifically because the seeded admin sees inline-editable cells
  // (title, author, …) that intercept clicks to open their editor instead of
  // navigating; the cover cell is non-editable, so its click bubbles to the
  // row's navigate handler — what a user clicking a non-interactive part of
  // the row gets.
  await getRow(page, TARGET.slug).getByTestId("ebook-cell-cover").click();
  // `/books/:uuid` — UUIDv5 in canonical 8-4-4-4-12 hyphenated form.
  await expect(page).toHaveURL(/\/books\/[0-9a-fA-F-]{36}$/);

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
  // Resolve the backend uuid the same way a real click would: read it out
  // of the same RPC the landing page consumes. Deep-linking by uuid keeps
  // this test independent of the landing page's row order.
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);

  await gotoReady(page, `/books/${uuid}`);

  // Title heading matches the fixture
  await expect(page.getByRole("heading", { level: 1, name: TARGET.title })).toBeVisible();

  // At least the first author is visible. Scoped to the dedicated authors
  // line because the breadcrumb falls back to the first author when the book
  // has no series, so a bare getByText(...) matches twice.
  await expect(page.getByTestId("book-authors")).toContainText(TARGET.authors[0]);

  // Breadcrumb navigation: "Home" link must be present inside the breadcrumb nav
  await expect(
    page.getByRole("navigation", { name: "breadcrumb" }).getByRole("link", { name: "Home" }),
  ).toBeVisible();

  // Format switcher renders one row per available format (F1.4).
  const switcher = page.getByTestId("format-switcher");
  await expect(switcher).toBeVisible();

  // All fixture books are EPUB; the EPUB row must exist with its badge and
  // the per-format CTAs grouped underneath. Use getByTestId rather than CSS
  // attribute/class locators per `04-playwright.md` ("semantic first").
  const epubRow = switcher.getByTestId("format-row-epub");
  await expect(epubRow).toBeVisible();
  await expect(epubRow.getByTestId("format-badge")).toHaveText("EPUB");

  // Read now routes into the F2.2 immersive reader (an enabled link to
  // /read/:uuid on web); Send-to-Kindle stays disabled until F4.x ships.
  const readBtn = epubRow.getByTestId("action-read");
  await expect(readBtn).toBeVisible();
  await expect(readBtn).toHaveAttribute("href", /\/read\//);
  const kindleBtn = epubRow.getByTestId("action-kindle");
  await expect(kindleBtn).toBeVisible();
  await expect(kindleBtn).toBeDisabled();

  // No M4B fixture in the ebook seed — the per-format Listen CTA must NOT
  // render. (Scoped to the format switcher; the hero "Listen" secondary
  // button only renders when both formats are present.)
  await expect(switcher.getByTestId("action-listen")).toHaveCount(0);

  // F3.2 ratings ship as the interactive hero rating card; F3.3 suggestions
  // renders the "Readers also enjoyed" strip below the metadata. (Its inner
  // state — connect message / pending / results — depends on whether a
  // Hardcover key is configured and on the external API, so we only assert the
  // section + heading are present here, not the resolved contents.)
  await expect(page.getByTestId("rating-stars")).toBeVisible();
  await expect(page.getByTestId("suggestions-strip")).toBeAttached();
  await expect(
    page.getByRole("heading", { name: "Suggested for you" }),
  ).toBeVisible();

  // Back link still navigates to landing
  const backLink = page.getByRole("link", { name: "Back to library" });
  await expect(backLink).toBeVisible();
  await backLink.click();
  await expect(page).toHaveURL(/\/$/);
  await expect(page.getByTestId("ebook-table")).toBeVisible();
});

// ---------------------------------------------------------------------------
// Action — Send to Kobo (F4.1 KEPUB download)
// ---------------------------------------------------------------------------

test("Send to Kobo downloads the book with a uuid-named file", async ({ page, request }) => {
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${uuid}`);

  const epubRow = page.getByTestId("format-switcher").getByTestId("format-row-epub");
  const koboBtn = epubRow.getByTestId("action-kobo");
  await expect(koboBtn).toBeVisible();
  await expect(koboBtn).toHaveAttribute("href", new RegExp(`/api/ebooks/${uuid}/kepub$`));

  // Clicking downloads the file (the `download` attribute keeps the page put).
  // The filename stem is the canonical book uuid; the extension is
  // `.kepub.epub` when kepubify converts, or `.epub` on fallback — assert the
  // uuid-embedded contract that F4.4's USB import relies on, tolerant of both.
  const [download] = await Promise.all([
    page.waitForEvent("download"),
    koboBtn.click(),
  ]);
  expect(download.suggestedFilename()).toMatch(
    new RegExp(`^${uuid}\\.(kepub\\.)?epub$`),
  );
});

// ---------------------------------------------------------------------------
// Action — star rating (F3.2)
// ---------------------------------------------------------------------------

test("rates a book a half-star, persists across reload, then un-rates", async ({ page, request }) => {
  // Use a Wirth book so the rating never collides with the alpha-focused
  // assertions elsewhere in this serial file.
  const uuid = await fetchBookUuidByTitle(request, WIRTH_LEAD.title);
  await gotoReady(page, `/books/${uuid}`);

  const stars = page.getByTestId("rating-stars");
  await expect(stars).toBeVisible();
  await expect(page.getByTestId("rating-meta")).toHaveText("Not rated yet");

  // Click the left half of the 5th star → 4.5 → POST set, meta shows "of 5".
  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/ratings/set", expectedStatus: 200 },
    async () => stars.getByRole("button", { name: "Rate 4.5 stars" }).click(),
  );
  await expect(page.getByTestId("rating-meta")).toContainText("4.5 of 5");

  // The saved rating survives a reload (the post-mount effect refetches it).
  await gotoReady(page, `/books/${uuid}`);
  await expect(page.getByTestId("rating-meta")).toContainText("4.5 of 5");

  // Re-clicking the active half-star clears the rating (un-rate) and cleans up
  // shared server state for the next run.
  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/ratings/clear", expectedStatus: 200 },
    async () =>
      page
        .getByTestId("rating-stars")
        .getByRole("button", { name: "Rate 4.5 stars" })
        .click(),
  );
  await expect(page.getByTestId("rating-meta")).toHaveText("Not rated yet");
});

test("surfaces an error and leaves the rating unchanged when the save fails", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, WIRTH_LEAD.title);
  await gotoReady(page, `/books/${uuid}`);

  await page.route("**/api/rpc/ratings/set", (route) =>
    route.fulfill({ status: 500, contentType: "text/plain", body: "rating exploded" }),
  );

  const stars = page.getByTestId("rating-stars");
  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/ratings/set", expectedStatus: 500 },
    async () => stars.getByRole("button", { name: "Rate 3 stars" }).click(),
  );

  // The optimistic pick rolls back and the status line reports the failure.
  await expect(page.getByTestId("rating-meta")).toHaveText("Couldn't save rating — try again");
});

test("breadcrumb author segment links to the author page", async ({ page, request }) => {
  // `alpha` has a single author (Ada Lovelace) and no series, so the
  // breadcrumb shape is Home > Ada Lovelace > Alpha and the author segment
  // must be a router link to /authors/:id.
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${uuid}`);

  const crumb = page.getByRole("navigation", { name: "breadcrumb" });
  const authorLink = crumb.getByRole("link", { name: TARGET.authors[0] });
  await expect(authorLink).toBeVisible();
  await authorLink.click();
  await expect(page).toHaveURL(/\/authors\/\d+$/);
});

test("breadcrumb series segment links to the series page", async ({ page, request }) => {
  // `beta` is "Beta in the Series" #1 of "Pioneers" — so the breadcrumb is
  // Home > Grace Hopper > Pioneers #1 > Beta in the Series. The series
  // segment must be a router link to /series/:id.
  const beta = FIXTURE_BOOKS.find((b) => b.slug === "beta")!;
  const uuid = await fetchBookUuidByTitle(request, beta.title);
  await gotoReady(page, `/books/${uuid}`);

  const crumb = page.getByRole("navigation", { name: "breadcrumb" });
  const seriesLink = crumb.getByRole("link", { name: /Pioneers/ });
  await expect(seriesLink).toBeVisible();
  await seriesLink.click();
  await expect(page).toHaveURL(/\/series\/\d+$/);
});

// ---------------------------------------------------------------------------
// Audiobook-only seed: re-seeds the server with the audiobook fixtures so
// the audio-only "Start listening" CTA can be exercised. Sharded into a
// `describe` block so the audiobook re-seed only runs for the listen-CTA
// test — the rest of the spec keeps the ebook library it set up above.
// ---------------------------------------------------------------------------

test.describe("audiobook-only seed", () => {
  test.beforeAll(async ({ request }) => {
    await seedAudiobookLibrary(
      request,
      audiobookFixturesDir(),
      AUDIOBOOK_BOOK_COUNT,
    );
  });

  test("Start listening navigates to /listen/:uuid for audio-only books", async ({
    page,
    request,
  }) => {
    // Pick an MP3 audiobook with no ebook companion — has_audio is true
    // and has_ebook is false, so the hero renders "Start listening" as
    // the primary CTA (the secondary "Listen" button only appears when
    // both formats coexist).
    const mp3Book = AUDIOBOOK_BOOKS.find(
      (b) => b.format === "MP3" && b.source === "generated",
    )!;
    const uuid = await fetchBookUuidByTitle(request, mp3Book.title);
    await gotoReady(page, `/books/${uuid}`);

    const startListening = page.getByTestId("start-listening");
    await expect(startListening).toBeVisible();
    await expect(startListening).toHaveAttribute(
      "href",
      new RegExp(`/listen/${uuid}$`),
    );

    // Clicking the primary CTA must SPA-navigate to the listen page.
    await startListening.click();
    await expect(page).toHaveURL(new RegExp(`/listen/${uuid}$`));
  });
});

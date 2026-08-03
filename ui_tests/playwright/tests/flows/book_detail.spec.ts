import type { APIRequestContext } from "@playwright/test";
import {
  AUDIOBOOK_BOOK_COUNT,
  AUDIOBOOK_BOOKS,
  MERGE_ONLY_TITLES,
  MERGE_PRIMARY,
  MERGE_SECONDARY,
} from "../fixtures/audiobooks";
import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { expect, test } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import {
  fetchBookUuidByTitle,
  getRow,
  switchToTableView,
} from "../utils/ebooks";
import { withLock } from "../utils/lock";
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

// Reserved for the read/unread-status tests: read state is per-(user, book)
// server state, so this book must be read by no other spec. `standalone-canyon`
// (Annie Easley) appears in no other flow.
const READ_STATUS_BOOK = FIXTURE_BOOKS.find(
  (b) => b.slug === "standalone-canyon",
)!;

// Reserved for the saved-passages tests: highlights are per-(user, book)
// server state, so seeding one here must not disturb another spec's counts.
// `standalone-garden` is read by no other flow, and the reader spec's own
// highlight books (beta / gamma / pioneers-3) stay untouched.
const PASSAGES_BOOK = FIXTURE_BOOKS.find(
  (b) => b.slug === "standalone-garden",
)!;

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

test("renders the book detail layout", async ({ page, request }) => {
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${uuid}`);

  // Hero — title heading is the H1.
  await expect(
    page.getByRole("heading", { level: 1, name: TARGET.title }),
  ).toBeVisible();

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
  await expect(startReading).toHaveAttribute(
    "href",
    new RegExp(`/read/${uuid}$`),
  );

  // Hero — breadcrumb landmark with Home link.
  const crumb = page.getByRole("navigation", { name: "breadcrumb" });
  await expect(crumb).toBeVisible();
  await expect(crumb.getByRole("link", { name: "Home" })).toBeVisible();

  // Body — "Passages you saved" section is always rendered. Only its
  // presence is asserted here: `journal.spec.ts` seeds a highlight on this
  // same book, so whether the list or the empty state fills it depends on
  // run order. The empty and populated states are covered by the
  // saved-passages action tests below, on a book reserved for them.
  await expect(
    page.getByRole("heading", { name: "Passages you saved" }),
  ).toBeVisible();
  await expect(page.getByTestId("highlights-section")).toBeVisible();

  // Body — "From the same hand" section heading is always rendered.
  await expect(
    page.getByRole("heading", { name: "From the same hand" }),
  ).toBeVisible();

  // Footer — Back to library link.
  await expect(
    page.getByRole("link", { name: "Back to library" }),
  ).toBeVisible();
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
  // Hedy Lamarr authors exactly one work across the *entire* fixture set —
  // the standalone `gamma` ebook, and (unlike Ada Lovelace) no audiobook. So
  // even when a parallel spec has seeded the audiobook library, her
  // author-fetch stays single-book: the row filters to empty and the page
  // renders `from-same-hand-empty`. `alpha`/Ada can't be used here — Ada also
  // authors "The Analytical Audiobook", which flips her to multi-book the
  // moment audiobooks are indexed, so this assertion raced audiobook seeding.
  const SOLO = FIXTURE_BOOKS.find((b) => b.slug === "gamma")!;
  const uuid = await fetchBookUuidByTitle(request, SOLO.title);
  await gotoReady(page, `/books/${uuid}`);

  // Wait for the heading to confirm the section rendered.
  await expect(
    page.getByRole("heading", { name: "From the same hand" }),
  ).toBeVisible();

  // Empty-state card must be visible; the populated row must be absent.
  await expect(page.getByTestId("from-same-hand-empty")).toBeVisible();
  await expect(page.getByTestId("from-same-hand")).toHaveCount(0);
});

// ---------------------------------------------------------------------------
// Pre-existing coverage — kept so the spec remains a single source of truth
// for the book detail page. These post-date the original landing-row entry
// from #297; the new tests above were added for #385.
// ---------------------------------------------------------------------------

test("navigates from a landing row to the detail page and back", async ({
  page,
}) => {
  await gotoReady(page, "/");
  await switchToTableView(page);

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
  // re-render — assert URL plus that the table view (persisted above) comes
  // back.
  await backLink.click();
  await expect(page).toHaveURL(/\/$/);
  await expect(page.getByTestId("ebook-table")).toBeVisible();
});

test("renders the detail contents for the selected book", async ({
  page,
  request,
}) => {
  // Resolve the backend uuid the same way a real click would: read it out
  // of the same RPC the landing page consumes. Deep-linking by uuid keeps
  // this test independent of the landing page's row order.
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);

  await gotoReady(page, `/books/${uuid}`);

  // Title heading matches the fixture
  await expect(
    page.getByRole("heading", { level: 1, name: TARGET.title }),
  ).toBeVisible();

  // At least the first author is visible. Scoped to the dedicated authors
  // line because the breadcrumb falls back to the first author when the book
  // has no series, so a bare getByText(...) matches twice.
  await expect(page.getByTestId("book-authors")).toContainText(
    TARGET.authors[0]!,
  );

  // Breadcrumb navigation: "Home" link must be present inside the breadcrumb nav
  await expect(
    page
      .getByRole("navigation", { name: "breadcrumb" })
      .getByRole("link", { name: "Home" }),
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
  // /read/:uuid on web); F4.3 Send-to-Kindle is now an enabled action button
  // (the send flow is exercised in its own action test below).
  const readBtn = epubRow.getByTestId("action-read");
  await expect(readBtn).toBeVisible();
  await expect(readBtn).toHaveAttribute("href", /\/read\//);
  const kindleBtn = epubRow.getByTestId("action-kindle");
  await expect(kindleBtn).toBeVisible();
  await expect(kindleBtn).toBeEnabled();

  // The hero CTA row condenses the per-device actions behind an "Export"
  // dropdown. It's closed by default; opening it reveals the EPUB download
  // link and the Send-to-Kindle button (enabled because the book has an
  // EPUB), which carries its own testid so it doesn't collide with the
  // per-format-row button above.
  const exportTrigger = page.getByTestId("hero-export");
  await expect(exportTrigger).toBeVisible();
  await expect(page.getByTestId("hero-export-panel")).toHaveCount(0);
  await exportTrigger.click();
  const exportPanel = page.getByTestId("hero-export-panel");
  await expect(exportPanel).toBeVisible();
  const downloadEpub = exportPanel.getByTestId("export-download-epub");
  await expect(downloadEpub).toHaveAttribute(
    "href",
    /\/api\/ebooks\/.+\/download$/,
  );
  await expect(downloadEpub).toHaveAttribute("download");
  const heroKindleBtn = exportPanel.getByTestId("hero-send-kindle");
  await expect(heroKindleBtn).toBeVisible();
  await expect(heroKindleBtn).toBeEnabled();
  // Send-to-Kobo is the interactive button (writes the KEPUB onto a plugged-in
  // Kobo on Chromium, or downloads it to copy over elsewhere); enabled because
  // the book has an EPUB. Its own testid keeps it distinct from the per-format
  // action-kobo above.
  const koboExport = exportPanel.getByTestId("hero-send-kobo");
  await expect(koboExport).toBeVisible();
  await expect(koboExport).toBeEnabled();
  // The audiobook download only appears for books with an audio file; the
  // ebook-only seed must not render it. Close the menu again via the scrim.
  // The scrim fills the viewport, so click a corner clear of the panel
  // (a center click lands on the panel and is intercepted).
  await expect(exportPanel.getByTestId("export-download-audio")).toHaveCount(0);
  await page
    .getByTestId("hero-export-scrim")
    .click({ position: { x: 10, y: 10 } });
  await expect(page.getByTestId("hero-export-panel")).toHaveCount(0);

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

  // Back link still navigates to landing (default Grid view)
  const backLink = page.getByRole("link", { name: "Back to library" });
  await expect(backLink).toBeVisible();
  await backLink.click();
  await expect(page).toHaveURL(/\/$/);
  await expect(page.getByTestId("lib-grid")).toBeVisible();
});

// Action — Send to Kobo (F4.1 KEPUB direct write / download fallback)
// ---------------------------------------------------------------------------

test("Send to Kobo delivers the book with a uuid-named file", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);

  // Force the download fallback path: without the File System Access API the
  // button fetches the KEPUB and triggers a normal browser download. The native
  // directory picker can't be driven headlessly (and isn't present in every
  // Chromium build), so nulling `showDirectoryPicker` makes the path
  // deterministic in CI. Must run before navigation so it applies to the page.
  await page.addInitScript(() => {
    Object.defineProperty(window, "showDirectoryPicker", {
      configurable: true,
      value: undefined,
    });
  });
  await gotoReady(page, `/books/${uuid}`);

  const epubRow = page
    .getByTestId("format-switcher")
    .getByTestId("format-row-epub");
  const koboBtn = epubRow.getByTestId("action-kobo");
  await expect(koboBtn).toBeVisible();
  await expect(koboBtn).toBeEnabled();

  // Clicking downloads the file (the fallback anchor keeps the page put). The
  // filename stem is the canonical book uuid; the extension is `.kepub.epub`
  // when kepubify converts, or `.epub` on fallback — assert the uuid-embedded
  // contract that F4.4's USB import relies on, tolerant of both.
  const [download] = await Promise.all([
    page.waitForEvent("download"),
    koboBtn.click(),
  ]);
  expect(download.suggestedFilename()).toMatch(
    new RegExp(`^${uuid}\\.(kepub\\.)?epub$`),
  );
});

// ---------------------------------------------------------------------------
// Action — Send to Kindle (F4.3)
// ---------------------------------------------------------------------------

test("sends the EPUB to Kindle and shows a sent status", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${uuid}`);

  const kindleBtn = page
    .getByTestId("format-row-epub")
    .getByTestId("action-kindle");
  await expect(kindleBtn).toBeEnabled();

  // Send is enqueue-and-poll: the POST returns a worker task id, then the button
  // polls the status endpoint. Mock both so the test never touches a real SMTP
  // relay or a configured Kindle email.
  await page.route("**/api/rpc/kindle/send", (route) => {
    if (route.request().method() === "POST") {
      return route.fulfill({
        status: 200,
        contentType: "application/json",
        body: "1",
      });
    }
    return route.continue();
  });
  await page.route("**/api/rpc/kindle/send/status", (route) => {
    if (route.request().method() === "POST") {
      return route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ status: "sent" }),
      });
    }
    return route.continue();
  });

  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/kindle/send",
      expectedBody: { book_uuid: uuid, file_id: null },
      expectedStatus: 200,
    },
    async () => kindleBtn.click(),
  );

  await expect(page.getByTestId("kindle-send-status")).toHaveText(
    "Sent to your Kindle.",
  );
  await expect(page.getByTestId("kindle-send-status")).toHaveClass(/success/);
});

test("shows an error when the Kindle send fails", async ({ page, request }) => {
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await gotoReady(page, `/books/${uuid}`);

  const kindleBtn = page
    .getByTestId("format-row-epub")
    .getByTestId("action-kindle");

  // Enqueue succeeds (returns a task id); the worker delivery then fails,
  // surfaced by the status poll. This is the path that previously hung the
  // button on "Sending…" instead of ever raising an error.
  await page.route("**/api/rpc/kindle/send", (route) => {
    if (route.request().method() === "POST") {
      return route.fulfill({
        status: 200,
        contentType: "application/json",
        body: "7",
      });
    }
    return route.continue();
  });
  await page.route("**/api/rpc/kindle/send/status", (route) => {
    if (route.request().method() === "POST") {
      return route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          status: "failed",
          message: "SMTP delivery failed",
        }),
      });
    }
    return route.continue();
  });

  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/kindle/send",
      expectedBody: { book_uuid: uuid, file_id: null },
      expectedStatus: 200,
    },
    async () => kindleBtn.click(),
  );

  // The error surfaces as a persistent toast (it does not auto-dismiss like the
  // success toast) and can be cleared via its dismiss button.
  await expect(page.getByTestId("kindle-send-status")).toHaveClass(/error/);
  await expect(page.getByTestId("kindle-send-status")).toContainText(
    "SMTP delivery failed",
  );
  await page.getByTestId("kindle-toast-dismiss").click();
  await expect(page.getByTestId("kindle-send-status")).toHaveCount(0);
});

// ---------------------------------------------------------------------------
// Action — star rating (F3.2)
// ---------------------------------------------------------------------------

test("hides other ratings when no one else has rated the book", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await page.route("**/api/rpc/ratings/others", (route) =>
    route.fulfill({ status: 200, contentType: "application/json", body: "[]" }),
  );

  await gotoReady(page, `/books/${uuid}`);

  await expect(page.getByTestId("other-ratings")).toHaveCount(0);
});

test("shows attributed ratings from other users", async ({ page, request }) => {
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  const twoDaysAgo = Math.floor(Date.now() / 1000) - 2 * 86_400;
  await page.route("**/api/rpc/ratings/others", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify([
        {
          user_id: 42,
          username: "reader",
          stars: 4.5,
          updated_at: twoDaysAgo,
        },
      ]),
    }),
  );

  await gotoReady(page, `/books/${uuid}`);

  const row = page.getByTestId("other-rating-row");
  await expect(row).toBeVisible();
  await expect(row).toContainText("R");
  await expect(row.getByTestId("other-rating-byline")).toHaveText(
    "reader rated 2 days ago",
  );
  await expect(row.getByTestId("other-rating-content")).toHaveCSS(
    "flex-direction",
    "column",
  );
  await expect(row.getByLabel("4.5 out of 5 stars")).toBeVisible();
});

test("rates a book a half-star, persists across reload, then un-rates", async ({
  page,
  request,
}) => {
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
    route.fulfill({
      status: 500,
      contentType: "text/plain",
      body: "rating exploded",
    }),
  );

  const stars = page.getByTestId("rating-stars");
  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/ratings/set", expectedStatus: 500 },
    async () => stars.getByRole("button", { name: "Rate 3 stars" }).click(),
  );

  // The optimistic pick rolls back and the status line reports the failure.
  await expect(page.getByTestId("rating-meta")).toHaveText(
    "Couldn't save rating — try again",
  );
});

test("renders the read-status control with three segments", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, READ_STATUS_BOOK.title);
  await gotoReady(page, `/books/${uuid}`);

  const control = page.getByTestId("read-status-control");
  await expect(control).toBeVisible();
  await expect(control.getByTestId("read-status-unread")).toBeVisible();
  await expect(control.getByTestId("read-status-reading")).toBeVisible();
  await expect(control.getByTestId("read-status-finished")).toBeVisible();
});

test("marks a book finished, persists across reload, then resets to unread", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, READ_STATUS_BOOK.title);
  await gotoReady(page, `/books/${uuid}`);

  const control = page.getByTestId("read-status-control");
  // Mark finished → POST set, the Finished segment activates, meta updates.
  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/read-status/set", expectedStatus: 200 },
    async () => control.getByTestId("read-status-finished").click(),
  );
  await expect(control.getByTestId("read-status-finished")).toHaveClass(
    /active/,
  );
  await expect(page.getByTestId("read-status-meta")).toContainText("Finished");

  // The saved state survives a reload (the post-mount effect refetches it).
  await gotoReady(page, `/books/${uuid}`);
  await expect(
    page.getByTestId("read-status-control").getByTestId("read-status-finished"),
  ).toHaveClass(/active/);

  // Reset to unread so the shared server state is clean for the next run.
  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/read-status/set", expectedStatus: 200 },
    async () =>
      page
        .getByTestId("read-status-control")
        .getByTestId("read-status-unread")
        .click(),
  );
  await expect(page.getByTestId("read-status-meta")).toHaveText("Not started");
});

test("reverts the read status and reports an error when the save fails", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, READ_STATUS_BOOK.title);
  await gotoReady(page, `/books/${uuid}`);

  // Establish a known baseline (reading), then force the next save to fail.
  const control = page.getByTestId("read-status-control");
  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/read-status/set", expectedStatus: 200 },
    async () => control.getByTestId("read-status-reading").click(),
  );
  await expect(control.getByTestId("read-status-reading")).toHaveClass(
    /active/,
  );

  await page.route("**/api/rpc/read-status/set", (route) =>
    route.fulfill({
      status: 500,
      contentType: "text/plain",
      body: "read-status exploded",
    }),
  );
  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/read-status/set", expectedStatus: 500 },
    async () => control.getByTestId("read-status-finished").click(),
  );

  // The optimistic pick rolls back to reading and the status line reports it.
  await expect(control.getByTestId("read-status-reading")).toHaveClass(
    /active/,
  );
  await expect(page.getByTestId("read-status-meta")).toHaveText(
    "Couldn't save — try again",
  );

  // Clean up shared state: drop the route override and reset to unread.
  await page.unroute("**/api/rpc/read-status/set");
  await expectMutation(
    page,
    { method: "POST", url: "/api/rpc/read-status/set", expectedStatus: 200 },
    async () => control.getByTestId("read-status-unread").click(),
  );
});

// ---------------------------------------------------------------------------
// Action — saved passages (highlights)
// ---------------------------------------------------------------------------

/// Seed one highlight on PASSAGES_BOOK and return its uuid, id and quote. The
/// quote is stamped so a re-run never collides with a leftover row on the
/// shared, persistent dev DB. `note` needs a follow-up PATCH — `CreateHighlight`
/// carries no note field, so passing one to the POST would be silently dropped.
async function seedPassage(
  request: APIRequestContext,
  cfi: string,
  note?: string,
): Promise<{ uuid: string; id: number; quote: string }> {
  const uuid = await fetchBookUuidByTitle(request, PASSAGES_BOOK.title);
  const quote = `saved passage ${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
  const created = await request.post("/api/highlights", {
    data: {
      book_uuid: uuid,
      epub_cfi_range: cfi,
      color: "violet",
      text: quote,
    },
  });
  expect(created.status(), "seed highlight").toBe(200);
  const { id } = (await created.json()) as { id: number };
  if (note !== undefined) {
    const patched = await request.patch(`/api/highlights/${id}/note`, {
      data: { note },
    });
    expect(patched.status(), "seed highlight note").toBe(204);
  }
  return { uuid, id, quote };
}

// Start from a known-empty book. These tests assert the empty state *returns*
// after a delete, so residue from an interrupted earlier run would fail them
// for the wrong reason.
test.beforeAll(async ({ request }) => {
  const uuid = await fetchBookUuidByTitle(request, PASSAGES_BOOK.title);
  const list = await request.get(`/api/highlights/book/${uuid}`);
  expect(list.status(), "list highlights").toBe(200);
  for (const h of (await list.json()) as { id: number }[]) {
    expect((await request.delete(`/api/highlights/${h.id}`)).status()).toBe(
      204,
    );
  }
});

test("lists a saved passage with its locator, note and date, then deletes it", async ({
  page,
  request,
}) => {
  const { uuid, quote } = await seedPassage(
    request,
    "epubcfi(/6/14[chap03]!/4/2,/1:0,/1:40)",
    "the line that stuck",
  );

  await gotoReady(page, `/books/${uuid}`);

  // The empty state must give way to the list.
  await expect(page.getByTestId("highlights-empty")).toHaveCount(0);
  const card = page.getByTestId("highlight-card").filter({ hasText: quote });
  await expect(card).toHaveCount(1);
  await expect(card.getByTestId("highlight-note")).toHaveText(
    "the line that stuck",
  );
  // The locator is derived from the CFI's spine step (/14 → section 7); the
  // date comes from the server-assigned created_at.
  await expect(card.getByTestId("highlight-meta")).toContainText("Section 7");
  await expect(card.getByTestId("highlight-meta")).toContainText("saved ");

  // Delete it — the RPC must fire and the card must leave the list.
  await expectMutation(
    page,
    {
      method: "POST",
      url: /\/api\/rpc\/highlights\/delete(?:\?|$)/,
      expectedStatus: 200,
    },
    async () => card.getByTestId("highlight-delete").click(),
  );
  await expect(page.getByText(quote)).toHaveCount(0);
  await expect(page.getByTestId("highlights-empty")).toBeVisible();
});

test("keeps the passage listed when the delete request fails", async ({
  page,
  request,
}) => {
  const { uuid, quote } = await seedPassage(
    request,
    "epubcfi(/6/6!/4/2,/1:0,/1:20)",
  );

  await gotoReady(page, `/books/${uuid}`);
  const card = page.getByTestId("highlight-card").filter({ hasText: quote });
  await expect(card).toHaveCount(1);

  await page.route("**/api/rpc/highlights/delete*", (route) =>
    route.fulfill({
      status: 500,
      contentType: "text/plain",
      body: "delete exploded",
    }),
  );
  await expectMutation(
    page,
    {
      method: "POST",
      url: /\/api\/rpc\/highlights\/delete(?:\?|$)/,
      expectedStatus: 500,
    },
    async () => card.getByTestId("highlight-delete").click(),
  );

  // The row is only dropped once the server confirms, so it survives.
  await expect(card).toHaveCount(1);

  // Clean up shared state: drop the override and delete for real.
  await page.unroute("**/api/rpc/highlights/delete*");
  await expectMutation(
    page,
    {
      method: "POST",
      url: /\/api\/rpc\/highlights\/delete(?:\?|$)/,
      expectedStatus: 200,
    },
    async () => card.getByTestId("highlight-delete").click(),
  );
  await expect(page.getByText(quote)).toHaveCount(0);
});

test("opens the reader at the passage via a cfi deep link", async ({
  page,
  request,
}) => {
  const cfi = "epubcfi(/6/4!/4/2,/1:0,/1:30)";
  const { uuid, id, quote } = await seedPassage(request, cfi);

  await gotoReady(page, `/books/${uuid}`);
  const card = page.getByTestId("highlight-card").filter({ hasText: quote });
  const open = card.getByTestId("highlight-open");

  // The link carries the percent-encoded CFI so the reader boots at the
  // passage rather than resuming wherever this book was last left off.
  // Assert the decoded contract, not the escaping style — the Rust encoder
  // escapes `!()` too, which `encodeURIComponent` leaves alone.
  const href = await open.getAttribute("href");
  expect(href).not.toBeNull();
  const linked = new URL(href as string, page.url());
  expect(linked.pathname).toBe(`/read/${uuid}`);
  expect(linked.searchParams.get("cfi")).toBe(cfi);

  await open.click();
  await expect(page).toHaveURL(
    (url) =>
      url.pathname === `/read/${uuid}` && url.searchParams.get("cfi") === cfi,
  );
  await expect(page.getByTestId("reader-viewer")).toBeVisible();

  // Clean up the seeded highlight so re-runs start from the empty state.
  expect((await request.delete(`/api/highlights/${id}`)).status()).toBe(204);
});

test("opens the quote-card editor in a modal and closes it again", async ({
  page,
  request,
}) => {
  const { uuid, id, quote } = await seedPassage(
    request,
    "epubcfi(/6/10!/4/2,/1:0,/1:25)",
  );

  await gotoReady(page, `/books/${uuid}`);
  const card = page.getByTestId("highlight-card").filter({ hasText: quote });
  await card.getByTestId("highlight-quote").click();

  // The shared quote-card editor mounts in the app modal shell with the
  // passage on the preview and the export actions available.
  const modal = page.getByTestId("quote-card-modal");
  await expect(modal).toBeVisible();
  await expect(
    modal.getByRole("heading", { name: "Make a quote card" }),
  ).toBeVisible();
  await expect(modal).toContainText(quote);
  await expect(modal.getByTestId("quote-download")).toBeVisible();
  await expect(modal.getByTestId("quote-copy-image")).toBeVisible();

  // Typography controls restyle the preview in place — no request fires, so
  // assert the rendered card rather than a network contract.
  const preview = modal.getByTestId("quote-preview");
  const previewBody = modal.getByTestId("quote-preview-body");
  await expect(preview).toHaveCSS("font-family", /Instrument Serif/);
  await modal.getByRole("button", { name: "Sans", exact: true }).click();
  await expect(preview).toHaveCSS("font-family", /Geist/);

  await expect(previewBody).toHaveCSS("font-style", "italic");
  await modal.getByRole("button", { name: "Italic", exact: true }).click();
  await expect(previewBody).toHaveCSS("font-style", "normal");
  await modal.getByRole("button", { name: "Bold", exact: true }).click();
  await expect(previewBody).toHaveCSS("font-weight", "700");
  await modal.getByRole("button", { name: "L", exact: true }).click();
  await expect(previewBody).toHaveCSS("font-size", "27px");

  // Close via the X — no mutation fires; the card stays listed.
  await modal.getByRole("button", { name: "Close" }).click();
  await expect(modal).toHaveCount(0);
  await expect(card).toHaveCount(1);

  // Clean up the seeded highlight so re-runs start from the empty state.
  expect((await request.delete(`/api/highlights/${id}`)).status()).toBe(204);
});

test("breadcrumb author segment links to the author page", async ({
  page,
  request,
}) => {
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

test("breadcrumb series segment links to the series page", async ({
  page,
  request,
}) => {
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

  // The merge-only MP3 pair. Merging them (F5.10 allows same-format merges)
  // is what produces a real multi-file `book_files` group for #1005's
  // file-picker coverage below. They are reserved for the merge tests
  // precisely so this mutation stays invisible to the specs running in
  // parallel against the same server — see `fixtures/audiobooks.ts`.
  const PRIMARY_MP3 = MERGE_PRIMARY;
  const SECOND_MP3 = MERGE_SECONDARY;

  // Read-only audiobook for the non-mutating CTA test below. It must NOT be
  // one of the merge pair: that test asserts the *absence* of a file picker,
  // which a concurrent merge would falsify.
  const SOLO_MP3 = AUDIOBOOK_BOOKS.find(
    (b) =>
      b.format === "MP3" &&
      b.source === "generated" &&
      !MERGE_ONLY_TITLES.includes(b.title),
  )!;

  test("Start listening navigates to /listen/:uuid for audio-only books", async ({
    page,
    request,
  }) => {
    // Pick an MP3 audiobook with no ebook companion — has_audio is true
    // and has_ebook is false, so the hero renders "Start listening" as
    // the primary CTA (the secondary "Listen" button only appears when
    // both formats coexist).
    const uuid = await fetchBookUuidByTitle(request, SOLO_MP3.title);
    await gotoReady(page, `/books/${uuid}`);

    const startListening = page.getByTestId("start-listening");
    await expect(startListening).toBeVisible();
    await expect(startListening).toHaveAttribute(
      "href",
      new RegExp(`/listen/${uuid}$`),
    );

    // AC2 (#1005): a single-file audiobook shows no file picker next to
    // the CTA — the picker only appears once a book has >1 file of the
    // format that CTA opens (see the merge-based test below).
    await expect(page.getByTestId("listen-file-picker-trigger")).toHaveCount(0);

    // Clicking the primary CTA must SPA-navigate to the listen page. Dioxus
    // serializes the optional `?file_id=` route param as a bare trailing `?`.
    await startListening.click();
    await expect(page).toHaveURL(new RegExp(`/listen/${uuid}\\??$`));
  });

  // ---------------------------------------------------------------------------
  // #1005 — Listen file picker for a multi-file (duplicate-format) audiobook
  // ---------------------------------------------------------------------------

  test("shows a Listen file picker after merging two same-format audiobooks, and picking a file navigates with its file_id", async ({
    page,
    request,
  }) => {
    // Serialize against merge.spec's audiobook merge: both mutate "The
    // Analytical Audiobook" on the shared per-shard server, so running them in
    // parallel workers corrupts this test's two-file state. The lock leaves the
    // fixture restored (via the finally undo) before releasing. `test.slow()`
    // covers the worst case of waiting out the other holder plus this run.
    test.slow();
    await withLock("audiobook-merge", async () => {
      const primaryUuid = await fetchBookUuidByTitle(
        request,
        PRIMARY_MP3.title,
      );
      const secondUuid = await fetchBookUuidByTitle(request, SECOND_MP3.title);

      // Arrange: merge SECOND into PRIMARY via the RPC directly (the merge
      // dialog UI itself is covered by merge.spec.ts) so PRIMARY ends up with
      // two MP3 `book_files` rows — the duplicate-format scenario #1005's
      // picker targets. `merge_log_id` is the undo handle used in `finally`.
      const mergeResp = await request.post("/api/rpc/merge-books", {
        data: { source_uuid: secondUuid, target_uuid: primaryUuid },
      });
      expect(mergeResp.status(), "POST /api/rpc/merge-books failed").toBe(200);
      const { merge_log_id: mergeLogId } = (await mergeResp.json()) as {
        merge_log_id: number;
      };

      try {
        await gotoReady(page, `/books/${primaryUuid}`);

        // AC1: a book with >1 file of the format the CTA opens shows a way
        // to pick which file to open before listening.
        const trigger = page.getByTestId("listen-file-picker-trigger");
        await expect(trigger).toBeVisible();
        await expect(trigger).toHaveText(/Start listening\s*▾/);
        await expect(page.getByTestId("listen-file-picker-panel")).toHaveCount(
          0,
        );
        await trigger.click();
        const panel = page.getByTestId("listen-file-picker-panel");
        await expect(panel).toBeVisible();
        await expect(page.getByTestId("listen-file-picker-heading")).toHaveText(
          "2 files · choose one",
        );
        await expect(panel.getByRole("link")).toHaveCount(2);
        await expect(panel.getByRole("link").first()).toContainText(
          "Audiobook ·",
        );
        await expect(panel.getByRole("link").first()).toContainText(
          /\d+(?:\.\d+)? [KMGT]?B/,
        );

        // Picking the second (non-default) file navigates to /listen/:uuid
        // with that file's id, and the manifest fetch carries the same id.
        const [manifestReq] = await Promise.all([
          page.waitForRequest((req) =>
            req
              .url()
              .includes(`/api/audiobooks/${primaryUuid}/manifest?file_id=`),
          ),
          panel.getByRole("link").nth(1).click(),
        ]);
        expect(manifestReq.url()).toMatch(
          new RegExp(`/api/audiobooks/${primaryUuid}/manifest\\?file_id=\\d+$`),
        );
        await expect(page).toHaveURL(
          new RegExp(`/listen/${primaryUuid}\\?file_id=\\d+$`),
        );
      } finally {
        // Undo — restores SECOND as its own book so later specs (and re-runs
        // of this one) see the pre-merge fixture state, mirroring
        // merge.spec.ts's own undo-as-cleanup pattern.
        const undoResp = await request.post("/api/rpc/merge-books/undo", {
          data: { merge_log_id: mergeLogId },
        });
        expect(undoResp.status(), "POST /api/rpc/merge-books/undo failed").toBe(
          200,
        );
      }
    });
  });

  // ---------------------------------------------------------------------------
  // Regression: picking a different part of the *already-playing* book must
  // switch parts. The app-root web driver used to react only to the book uuid
  // (constant across a book's files), so re-picking a file on the active book
  // silently kept part 1. Assert a fresh manifest fetch fires for the newly
  // selected file_id even when the uuid is unchanged.
  // ---------------------------------------------------------------------------

  test("re-picking a different part of the already-active audiobook reloads onto that part", async ({
    page,
    request,
  }) => {
    test.slow();
    await withLock("audiobook-merge", async () => {
      const primaryUuid = await fetchBookUuidByTitle(
        request,
        PRIMARY_MP3.title,
      );
      const secondUuid = await fetchBookUuidByTitle(request, SECOND_MP3.title);

      const mergeResp = await request.post("/api/rpc/merge-books", {
        data: { source_uuid: secondUuid, target_uuid: primaryUuid },
      });
      expect(mergeResp.status(), "POST /api/rpc/merge-books failed").toBe(200);
      const { merge_log_id: mergeLogId } = (await mergeResp.json()) as {
        merge_log_id: number;
      };

      try {
        // The two audio `book_files` rows the merge produced, in file-picker
        // order (`GET /api/ebooks/:uuid` returns them by ordinal).
        const detailResp = await request.get(`/api/ebooks/${primaryUuid}`);
        expect(detailResp.status(), "GET /api/ebooks/:uuid failed").toBe(200);
        const detail = (await detailResp.json()) as {
          book_files: { id: number; format: string }[];
        };
        const audioIds = detail.book_files
          .filter((f) => /^(mp3|m4b|m4a)$/i.test(f.format))
          .map((f) => f.id);
        expect(audioIds.length, "expected two audio files after merge").toBe(2);

        // The whole flow below is one single-page-app session: a single full
        // load of the book-detail page, then every navigation is a real Dioxus
        // `<Link>` (the file picker) or an in-app history transition
        // (`goBack`). The `__repickSentinel` on `window` is set once after that
        // initial load and must survive to the end — a full page load anywhere
        // clears it, and a full reload refetches the manifest regardless of the
        // app-root driver fix, so the sentinel is what proves the *driver* (not
        // a reload) rebooted onto the newly-picked part.
        //
        // (A raw injected `<a>` is NOT usable here: dioxus-web routes clicks
        // through its `Link` components, not a document-level delegated
        // listener, so an injected anchor full-page-loads and defeats the
        // sentinel. The real picker links are the only faithful in-app nav.)
        const manifestFor = (fid: string) =>
          new RegExp(
            `/api/audiobooks/${primaryUuid}/manifest\\?file_id=${fid}$`,
          );
        const pickerLinks = async () => {
          await page.getByTestId("listen-file-picker-trigger").click();
          await expect(
            page.getByTestId("listen-file-picker-panel"),
          ).toBeVisible();
          return page.getByTestId("listen-file-picker-panel").getByRole("link");
        };
        const fileIdOf = (manifestUrl: string) =>
          new URL(manifestUrl).searchParams.get("file_id");

        await gotoReady(page, `/books/${primaryUuid}`);
        await page.evaluate(() => {
          (
            window as unknown as { __repickSentinel?: boolean }
          ).__repickSentinel = true;
        });

        // Pick the FIRST part via the picker → SPA-navigate to /listen and make
        // the book the active playback book (uuid now set on the app-root
        // PlaybackState).
        const [firstManifest] = await Promise.all([
          page.waitForRequest((req) => manifestFor("\\d+").test(req.url())),
          (await pickerLinks()).nth(0).click(),
        ]);
        const firstFileId = fileIdOf(firstManifest.url());
        await expect(page).toHaveURL(
          new RegExp(`/listen/${primaryUuid}\\?file_id=\\d+$`),
        );

        // Back to the detail page in-app (history popstate — no full load).
        await page.goBack();
        await expect(page).toHaveURL(new RegExp(`/books/${primaryUuid}$`));

        // Re-pick the SECOND part of the *already-active* book. Before the fix
        // the driver keyed only on the (unchanged) uuid, so this fired no fresh
        // manifest and stayed on part 1.
        const [secondManifest] = await Promise.all([
          page.waitForRequest((req) => manifestFor("\\d+").test(req.url()), {
            timeout: 10_000,
          }),
          (await pickerLinks()).nth(1).click(),
        ]);
        const secondFileId = fileIdOf(secondManifest.url());
        expect(
          secondFileId,
          "re-pick must reboot onto a different part",
        ).not.toBe(firstFileId);

        // The whole session stayed client-side — if any hop fully reloaded, the
        // manifest fetch above would prove nothing.
        const stayedInApp = await page.evaluate(
          () =>
            (window as unknown as { __repickSentinel?: boolean })
              .__repickSentinel === true,
        );
        expect(
          stayedInApp,
          "re-pick nav must stay in-app (no full page load)",
        ).toBe(true);
      } finally {
        const undoResp = await request.post("/api/rpc/merge-books/undo", {
          data: { merge_log_id: mergeLogId },
        });
        expect(undoResp.status(), "POST /api/rpc/merge-books/undo failed").toBe(
          200,
        );
      }
    });
  });
});

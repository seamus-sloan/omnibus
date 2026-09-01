// Reading-stats page (/stats): layout, the Week / Month / Year / Lifetime
// pills in the windowed band's own header, the standing band's immunity to
// period changes, the User / Library scope switch, and the zero-activity
// empty state.
//
// The page's load-bearing contract is the windowed / standing split: the pills
// govern everything under "In this window" and nothing under "Outside the
// window" or in the hero, and several tests below exist only to hold that
// line.
//
// Sessions are seeded once in beforeAll — before the first /stats visit — so
// the server-side 60s stats cache never captures an empty summary for the
// shared test user. The session recorder skips uuids that don't resolve to an
// indexed book, so the library is seeded first and sessions ride a real
// fixture uuid.

import type { APIRequestContext, Page } from "@playwright/test";
import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { expect, test } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import { fetchBookUuidByTitle } from "../utils/ebooks";
import { expectNavVisible, gotoReady } from "../utils/nav";
import { fixturesDir, seedLibrary } from "../utils/seed";

// Serial: the beforeAll seed mutates shared server state (library reindex +
// session inserts). Under the suite's fullyParallel default it would re-run
// per worker and the concurrent writes 500 on SQLite's write lock.
test.describe.configure({ mode: "serial" });

/**
 * Genre assigned to this spec's donut feeder. Deliberately unusual so a
 * future fixture can't collide with it and change the slice count.
 */
const DONUT_GENRE = "Stats Spec Gothic";

/** Late-2023 anchor: inside Lifetime, outside the week/month/year windows. */
const OLD_SESSION_AT = 1_700_000_000;

/** The period pill for one range — they live in the windowed band's header. */
function periodPill(page: Page, range: "week" | "month" | "year" | "all_time") {
  return page.getByTestId(`stats-range-${range}`);
}

/**
 * Switch the window and wait for the refetch it triggers, so a caller can
 * assert against the new summary rather than racing the old one.
 */
async function selectPeriod(
  page: Page,
  range: "week" | "month" | "year" | "all_time",
) {
  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/stats",
      // The testid *is* the wire name (`StatsRange::as_query`), so the pill
      // and the request it fires can't drift apart. `utc_offset_minutes` is
      // the browser's own offset, which the config pins to UTC (`timezoneId`)
      // so it is deterministically 0 — it is where the server cuts the
      // answer's day boundaries (rule 10).
      expectedBody: { range, utc_offset_minutes: 0 },
      expectedStatus: 200,
    },
    async () => periodPill(page, range).click(),
  );
  await expect(periodPill(page, range)).toHaveAttribute("aria-pressed", "true");
}

/** Move to the Library scope, where the shelf's own figures live. */
async function openLibraryScope(page: Page) {
  await page.getByTestId("stats-scope-tab-library").click();
  await expect(page.getByTestId("stats-scope-library")).toBeVisible();
}

test.beforeAll(async ({ request }) => {
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
  const uuid = await fetchBookUuidByTitle(request, FIXTURE_BOOKS[0]!.title);

  // The genre donut shares by book count over active books that have a
  // *genre*. Nothing scans one, so it has to be assigned here. Dracula is
  // this spec's designated donut feeder; only its `genres` key is written,
  // so the "Vampires" tag search.spec.ts reads stays untouched.
  const genredUuid = await fetchBookUuidByTitle(request, "Dracula");
  const genreResp = await request.post(`/api/ebooks/${genredUuid}/overrides`, {
    data: { genres: [DONUT_GENRE] },
  });
  expect(genreResp.status(), "seeding the donut genre failed").toBe(200);

  const now = Math.floor(Date.now() / 1000);
  const session = (
    bookUuid: string,
    startedAt: number,
    secs: number,
    format: "epub" | "audio",
  ) => ({
    book_uuid: bookUuid,
    format,
    started_at: startedAt,
    ended_at: startedAt + secs,
    progress_units: secs,
    // The time-pattern strips bucket on the offset the recording device
    // stamped at capture; a report without one is excluded from them by
    // design, so these seeds carry a fixed non-UTC zone.
    utc_offset_minutes: -420,
  });
  const resp = await request.post("/api/progress/sessions", {
    data: [
      session(uuid, now - 60, 600, "epub"),
      session(uuid, OLD_SESSION_AT, 900, "epub"),
      session(uuid, now - 120, 900, "audio"),
      session(genredUuid, now - 300, 300, "epub"),
    ],
  });
  expect(resp.status(), "seeding stats sessions failed").toBe(200);
  const body = (await resp.json()) as { recorded: number };
  expect(body.recorded, "session seeds were skipped").toBe(4);

  // Feed the headline tiles: a star rating and a finished (100%) journal.
  const rating = await request.post("/api/ratings", {
    data: { book_uuid: uuid, stars: 4.5 },
  });
  expect(rating.status(), "seeding a rating failed").toBe(200);
  const journal = await request.post("/api/journals", {
    data: {
      book_uuid: uuid,
      body_md: "Finished it — stats spec seed.",
      progress: 100,
    },
  });
  expect(journal.status(), "seeding a finished journal failed").toBe(200);
});

test("renders the stats page layout", async ({ page }) => {
  await gotoReady(page, "/");
  await expectNavVisible(page);
  await page
    .getByRole("navigation", { name: "Primary" })
    .getByRole("link", { name: "Stats" })
    .click();
  await expect(page).toHaveURL(/\/stats$/);
  await page.waitForLoadState("networkidle");

  // The hero leads with the run the reader is on — a standing figure, above
  // the switcher entirely.
  await expect(page.getByTestId("stats-hero")).toBeVisible();
  await expect(page.getByTestId("stats-current-streak")).toContainText(/\d/);

  // The scope switch, then the windowed band with its own pills. All four
  // periods are offered, Month pre-selected (the default range).
  await expect(page.getByTestId("stats-scope-switch")).toBeVisible();
  await expect(page.getByTestId("stats-scope-user")).toBeVisible();
  for (const range of ["week", "month", "year", "all_time"] as const) {
    await expect(periodPill(page, range)).toBeVisible();
  }
  await expect(periodPill(page, "month")).toHaveAttribute(
    "aria-pressed",
    "true",
  );
  // The pills sit under a label that says what they govern, and beside a
  // window label that says what the current one covers.
  await expect(page.getByTestId("stats-window-head")).toContainText(
    "In this window",
  );
  await expect(page.getByTestId("stats-window-label")).toContainText(
    /month to date$/,
  );

  await expect(page.getByTestId("stats-period-section")).toBeVisible();
  // The reading clock renders in both its states (the dial or the
  // no-local-time note), so its presence is structural.
  await expect(page.getByTestId("stats-when")).toBeVisible();
  // The dial is the live end of the whole local-time path — the seeded
  // reports' `utc_offset_minutes` reaching the session column, the shifted
  // rollup, and the wire fields. Every other assertion in this file mocks
  // `/api/rpc/stats`, so without this one a serde rename or a dropped bind
  // would leave the suite green with the card stuck in its empty state.
  await expect(page.getByTestId("stats-when-empty")).toHaveCount(0);
  await expect(page.getByTestId("stats-clock-dial")).toBeVisible();
  await expect(page.getByTestId("stats-when-weekdays")).toBeVisible();

  // The standing band is labelled by what the pills *can't* reach, and
  // carries no accent rule of its own — the absence is the signal.
  await expect(page.getByText("Outside the window")).toBeVisible();
  await expect(page.getByTestId("stats-alltime-section")).toBeVisible();
});

test("headline tiles render finished, avg rating, pages, and listening", async ({
  page,
}) => {
  await gotoReady(page, "/stats");

  // Values are asserted by shape, not exact numbers — other specs sharing the
  // test user can add sessions/ratings/journals; the arithmetic is covered by
  // the db::stats unit tests.
  await expect(page.getByTestId("stats-tile-finished")).toContainText(/\d/);
  await expect(page.getByTestId("stats-tile-finished")).toContainText(
    "Finished",
  );
  await expect(page.getByTestId("stats-tile-avg-rating")).toContainText(
    /\d\.\d\s*★/,
  );
  // #2139: the tile counts pages *turned* in the window, not the length of the
  // books finished in it, so the finished journal seeded in beforeAll no longer
  // gives it a value — only real position writes do, and this spec deliberately
  // makes none. They would join the Continue fan the landing specs assert on,
  // and land on a book those specs read. Both a digit and the em-dash are
  // legitimate here; the label is the contract this test pins, and the
  // arithmetic lives in the db::stats unit tests.
  const pages = page.getByTestId("stats-tile-pages");
  await expect(pages).toContainText("Pages read");
  await expect(pages).toContainText(/\d|—/);
  await expect(page.getByTestId("stats-tile-listening")).toContainText(
    /\d+\s*(m|h)/,
  );
});

test("the Pages read drill-in explains its coverage and states the cutover", async ({
  page,
}) => {
  await gotoReady(page, "/stats");

  await page.getByTestId("stats-tile-pages").click();
  const drillIn = page.getByTestId("stats-drill-in");
  await expect(drillIn).toBeVisible();
  await expect(drillIn).toContainText("Pages read");

  // AC6: the panel renders content rather than opening to nothing, whatever
  // the window holds.
  await expect(page.getByTestId("stats-drill-pages-note")).toBeVisible();
  // AC9: page progress is differenced from stored positions and none exist
  // before the ledger began, so the tile changes meaning at a date — which the
  // UI states rather than leaving it as an unexplained discontinuity.
  await expect(page.getByTestId("stats-drill-pages-cutover")).toContainText(
    /Page tracking began \d{4}-\d{2}-\d{2}/,
  );

  await page.getByTestId("stats-drill-close").click();
  await expect(drillIn).toHaveCount(0);
});

test("a tile's grip opens its drill-in and the close button dismisses it", async ({
  page,
}) => {
  await gotoReady(page, "/stats");

  await page.getByTestId("stats-tile-avg-rating").click();
  const drillIn = page.getByTestId("stats-drill-in");
  await expect(drillIn).toBeVisible();
  await expect(drillIn).toContainText("Avg rating");
  // Every drill-in shows a delta chip (or the "not enough data" fallback) and
  // a trend chart — AC2.
  await expect(page.getByTestId("stats-drill-delta")).toBeVisible();
  await expect(page.getByTestId("stats-drill-trend")).toBeVisible();
  // Avg rating also carries the distribution, or its empty state when nothing
  // was rated in the window — other specs write ratings on the shared fixture,
  // so which one shows isn't ours to pin here.
  await expect(
    page
      .getByTestId("stats-drill-histogram")
      .or(page.getByTestId("stats-drill-histogram-empty")),
  ).toBeVisible();

  await page.getByTestId("stats-drill-close").click();
  await expect(drillIn).toHaveCount(0);

  // The selected period is untouched by opening/closing (AC4).
  await expect(periodPill(page, "month")).toHaveAttribute(
    "aria-pressed",
    "true",
  );
});

test("the Avg rating drill-in charts every half-star bucket on a star axis", async ({
  page,
}) => {
  // Route-mocked so the buckets are pinned: the shared fixture's ratings are
  // written by other specs, so asserting bar labels against live data would
  // race them.
  const histogram = Array.from({ length: 10 }, (_, i) => ({
    half_stars: i + 1,
    books: i === 6 ? 2 : i === 9 ? 3 : 0,
  }));
  await page.route("**/api/rpc/stats", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        range: "month",
        reading_seconds: 600,
        listening_seconds: 0,
        avg_stars: 4.4,
        sessions: 1,
        active_days: 1,
        longest_streak_days: 1,
        current_streak_days: 1,
        busiest_week_start: null,
        busiest_week_seconds: 600,
        books_finished: 1,
        heatmap: [],
        top_authors: [],
        top_tags: [],
        finished_books: [],
        rating_histogram: histogram,
      }),
    }),
  );

  await gotoReady(page, "/stats");
  await page.getByTestId("stats-tile-avg-rating").click();

  // Ten columns — empty buckets keep their place, or the shape lies — labelled
  // in stars rather than the stored 1..=10 half-star scale.
  const chart = page.getByTestId("stats-drill-histogram");
  await expect(chart).toBeVisible();
  await expect
    .poll(() => chart.getByTestId("stats-drill-bar-label").allInnerTexts())
    .toEqual(["0.5", "1", "1.5", "2", "2.5", "3", "3.5", "4", "4.5", "5"]);
  // The tallest bucket carries its book count on hover.
  await expect(chart.locator('[title="5 ★ · 3 books"]')).toBeVisible();
});

test("the Pages drill-in reports a reading rate, and says so when it can't", async ({
  page,
}) => {
  // Route-mocked, like the histogram above: the rate is computed off finished
  // books and recorded reading time on the shared fixture, both of which other
  // specs write. Two fulfilments in one test so the em-dash branch is pinned
  // against the same page.
  // Every key without `#[serde(default)]` on the Rust struct has to be here,
  // or the client fails the whole decode and renders its error state.
  const summary = (pagesPerHour: number | null) => ({
    range: "month",
    reading_seconds: 600,
    listening_seconds: 0,
    sessions: 1,
    active_days: 1,
    longest_streak_days: 1,
    busiest_week_start: null,
    busiest_week_seconds: 600,
    books_finished: 1,
    heatmap: [],
    top_authors: [],
    top_tags: [],
    finished_books: [],
    pages_read: 412,
    pages_per_hour: pagesPerHour,
  });

  await page.route("**/api/rpc/stats", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(summary(32.6)),
    }),
  );
  await gotoReady(page, "/stats");
  await page.getByTestId("stats-tile-pages").click();

  // Whole pages above ten an hour — the decimal would dress an estimate as a
  // measurement — and labelled an estimate, like the tile it expands.
  const rate = page.getByTestId("stats-drill-pages-rate");
  await expect(rate).toBeVisible();
  await expect(rate).toContainText("33");
  await expect(rate).toContainText("est. pages an hour");

  await page.getByTestId("stats-drill-close").click();

  // No rate is a "can't tell", never a zero: a reader whose finished books
  // carry no recorded time has not read at 0 pages an hour.
  await page.route("**/api/rpc/stats", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(summary(null)),
    }),
  );
  await gotoReady(page, "/stats");
  await page.getByTestId("stats-tile-pages").click();

  // The copy names both halves — either a missing length or missing time
  // produces this state, and the tile must not blame only one of them.
  await expect(page.getByTestId("stats-drill-pages-rate")).toContainText(
    "both a measurable length and recorded reading time",
  );
});

test("the superlatives card names each standout, and omits the ones it can't", async ({
  page,
}) => {
  // Route-mocked: the superlatives rank over finished books and recorded
  // sessions on the shared fixture, which other specs write. Every key without
  // `#[serde(default)]` on the Rust struct has to be present or the client
  // fails the whole decode.
  const base = {
    range: "month",
    reading_seconds: 600,
    listening_seconds: 0,
    sessions: 1,
    active_days: 1,
    longest_streak_days: 1,
    busiest_week_start: "2023-11-13",
    busiest_week_seconds: 14_400,
    books_finished: 2,
    heatmap: [],
    top_authors: [{ name: "Ursula K. Le Guin", seconds: 3600 }],
    top_tags: [],
    finished_books: [],
  };
  await page.route("**/api/rpc/stats", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        ...base,
        superlatives: {
          longest_book: {
            book_uuid: "u1",
            title: "Doorstopper",
            author: "A. Writer",
            value: 900,
          },
          fastest_read: {
            book_uuid: "u2",
            title: "Sprint",
            author: null,
            value: 3,
          },
        },
      }),
    }),
  );
  await gotoReady(page, "/stats");

  const card = page.getByTestId("stats-superlatives");
  await expect(card).toBeVisible();
  await expect(card).toContainText("Doorstopper");
  await expect(card).toContainText("900 pages");
  await expect(card).toContainText("in 3 days");
  // The busiest week and the most-read author come off fields the payload has
  // always carried and the web page never drew.
  await expect(card).toContainText("Week of 13 Nov 2023");
  await expect(card).toContainText("Ursula K. Le Guin");
  // Absent superlatives cost their row, not an em-dash.
  await expect(card).not.toContainText("Shortest book");
  await expect(card).not.toContainText("Longest sitting");
  // The fastest read is a lower bound, and says so.
  await expect(page.getByTestId("stats-superlatives-note")).toContainText(
    "tracked session",
  );

  // No superlative at all, and no busiest week: the card is absent rather
  // than an empty heading.
  await page.route("**/api/rpc/stats", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        ...base,
        busiest_week_start: null,
        busiest_week_seconds: 0,
        top_authors: [],
      }),
    }),
  );
  await gotoReady(page, "/stats");
  // Wait on a period-owned element, not the section: until the period fetch
  // resolves, PeriodSummary renders a bare placeholder that is visible and
  // carries no card — so asserting absence here would pass on the loading
  // state and miss the regression this half of the test exists to catch.
  await expect(page.getByTestId("stats-tile-finished")).toBeVisible();
  await expect(page.getByTestId("stats-superlatives")).toHaveCount(0);
});

test("the Finished drill-in lists the books completed in the window", async ({
  page,
}) => {
  await gotoReady(page, "/stats");

  await page.getByTestId("stats-tile-finished").click();
  const list = page.getByTestId("stats-drill-finished-list");
  await expect(list).toBeVisible();
  await expect(list).toContainText(FIXTURE_BOOKS[0]!.title);

  // Clicking the scrim (outside the sheet/modal) also dismisses it.
  await page.getByTestId("stats-drill-in").click({ position: { x: 4, y: 4 } });
  await expect(page.getByTestId("stats-drill-in")).toHaveCount(0);
});

test("switching the period re-queries and relabels the window", async ({
  page,
}) => {
  await gotoReady(page, "/stats");

  await selectPeriod(page, "week");

  // The window label follows the pills — it is the sentence that says what
  // "this window" currently means.
  await expect(page.getByTestId("stats-window-label")).toContainText(
    /^Week of \d{1,2} \w{3} \d{4} · to date$/,
  );
  await expect(periodPill(page, "month")).toHaveAttribute(
    "aria-pressed",
    "false",
  );

  // Lifetime has no previous window, so every tile drops its comparison
  // rather than measuring against a zero nobody recorded.
  await selectPeriod(page, "all_time");
  await expect(page.getByTestId("stats-window-label")).toContainText(
    "Everything you have tracked",
  );
  await expect(page.getByTestId("stats-tile-finished-delta")).toHaveCount(0);
});

test("the heatmap and genre donut render from seeded activity", async ({
  page,
}) => {
  await gotoReady(page, "/stats");

  // Heatmap: trailing-year grid with the coverage figures and the record in
  // its header, and at least one active cell — keyed off the cell tooltip
  // ("… on YYYY-MM-DD", which only active cells carry) rather than the
  // intensity CSS classes.
  const heatmap = page.getByTestId("stats-heatmap");
  await expect(heatmap).toBeVisible();
  await expect(page.getByTestId("stats-days-read")).toContainText("days read");
  // The record stands here; the run the reader is *on* leads the hero, and a
  // second copy two bands apart is where two surfaces start to disagree.
  await expect(heatmap.getByTestId("stats-longest-streak")).toContainText(
    "best run",
  );
  await expect(heatmap.getByTestId("stats-current-streak")).toHaveCount(0);
  expect(await heatmap.locator('[title*=" on "]').count()).toBeGreaterThan(0);

  // Donut: the seeded genre appears in the legend with a percentage; the
  // center counts the books the ring actually describes ("tagged"), not every
  // active book. This spec seeds activity on an ungenred book too, so the
  // untagged disclosure always renders — matched by pattern because the suite
  // shares one server and other specs contribute their own active books.
  const donut = page.getByTestId("stats-genre-donut");
  await expect(donut).toBeVisible();
  await expect(donut).toContainText(DONUT_GENRE);
  await expect(donut).toContainText("%");
  await expect(donut).toContainText("tagged");
  await expect(page.getByTestId("stats-donut-untagged")).toHaveText(
    /^\+\d+ books? without a genre$/,
  );

  // The read-vs-listened split rides the same card's foot: the ring says what
  // was read and the bars say how, and splitting them across two cards left a
  // reader comparing them to answer one question.
  const split = donut.getByTestId("stats-format-split");
  await expect(split).toContainText("Read");
  await expect(split).toContainText("Listened");
});

test("the length distribution buckets finished books and never hides the unknown ones", async ({
  page,
}) => {
  // Route-mocked: the bars depend on what the shared fixture user has finished
  // in the window, which other specs move.
  await page.route("**/api/rpc/stats", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        range: "month",
        reading_seconds: 600,
        listening_seconds: 0,
        sessions: 1,
        active_days: 1,
        longest_streak_days: 1,
        current_streak_days: 1,
        busiest_week_start: null,
        busiest_week_seconds: 600,
        books_finished: 6,
        heatmap: [],
        top_authors: [],
        top_tags: [],
        finished_books: [],
        length_buckets: [
          { label: "Under 300", books: 3 },
          { label: "300–499", books: 2 },
          { label: "500+", books: 0 },
          { label: "Unknown", books: 1 },
        ],
      }),
    }),
  );

  await gotoReady(page, "/stats");

  // How long the finished books were is a fact *about* the Finished tile, not
  // a peer of it, so it lives inside that tile's drill-in.
  await page.getByTestId("stats-tile-finished").click();
  const card = page.getByTestId("stats-length-split");
  await expect(card).toBeVisible();
  // Every bucket the server sent is rendered, including the empty one and —
  // the point of the bucket — the unmeasurable one.
  for (const label of ["Under 300", "300–499", "500+", "Unknown"]) {
    await expect(card).toContainText(label);
  }
  // Counts, not shares: "3 books" needs no denominator to be read.
  await expect(card.getByTestId("stats-length-row").first()).toContainText("3");
});

test("the books-per-month chart renders twelve bars with the current month highlighted", async ({
  page,
}) => {
  await gotoReady(page, "/stats");

  const chart = page.getByTestId("stats-monthly-chart");
  await expect(chart).toBeVisible();
  await expect(chart).toContainText("avg");

  const bars = chart.getByTestId("stats-monthly-bar");
  await expect(bars).toHaveCount(12);
  // The trailing (current) month is the last bar and carries the highlight.
  await expect(bars.last()).toHaveClass(/current/);
});

test("the library-size card states each total with its coverage", async ({
  page,
}) => {
  // Route-mocked: the real totals depend on how far the word-count backfill
  // has run against the shared fixture library, which no spec can pin.
  await page.route("**/api/rpc/library-size", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        books: 1510,
        words: { total: 412_000_000, books: 1204 },
        pages: { total: 1_600_000, books: 1204 },
        // Nothing probed yet — the figure is absent, never a zero.
        listening_seconds: { total: 0, books: 0 },
      }),
    }),
  );
  await gotoReady(page, "/stats");
  await openLibraryScope(page);

  const card = page.getByTestId("stats-library-size");
  await expect(card).toBeVisible();
  // The scope reads as a sentence before it reads as figures.
  await expect(card).toContainText("1,510 books.");
  await expect(card).toContainText("412 million words");
  await expect(card).toContainText("412M");
  await expect(card).toContainText("1.6M");
  // Never a bare total: the denominator is what makes the number a fact.
  await expect(card).toContainText("across 1,204 of 1,510 books");
  await expect(card.getByTestId("stats-library-figure")).toHaveCount(2);

  // The scope switch says these figures are the shelf's, not the reader's —
  // and the pills are absent here rather than present and inert.
  await expect(page.getByTestId("stats-scope-switch")).toContainText(
    "not period-scoped",
  );
  await expect(periodPill(page, "week")).toHaveCount(0);
});

test("a library measured for nothing renders no size card at all", async ({
  page,
}) => {
  await page.route("**/api/rpc/library-size", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ books: 40 }),
    }),
  );
  await gotoReady(page, "/stats");
  await openLibraryScope(page);

  // Three zeroes would read as a claim about the collection rather than about
  // the backfill.
  await expect(page.getByTestId("stats-scope-library")).toBeVisible();
  await expect(page.getByTestId("stats-library-size")).toHaveCount(0);
});

// A composition with one dimension deliberately absent, so the empty-state
// assertion has something to bite on.
const COMPOSITION = {
  books: 1510,
  ghosted_books: 4,
  formats: {
    slices: [
      { label: "EPUB", books: 1400 },
      { label: "M4B", books: 180 },
    ],
    // 1,580 placements over 1,510 books: seventy are held in both formats.
    coverage: { total: 1580, books: 1510 },
  },
  languages: {
    slices: [{ label: "eng", books: 1180 }],
    coverage: { total: 1180, books: 1180 },
  },
  // No publisher metadata anywhere — the AC7 case.
  publishers: { slices: [], coverage: { total: 0, books: 0 } },
  decades: {
    slices: [
      { label: "1990s", books: 200 },
      { label: "2000s", books: 620 },
    ],
    coverage: { total: 820, books: 820 },
  },
  genres: {
    slices: [
      { label: "Fantasy", books: 40 },
      { label: "Horror", books: 22 },
    ],
    // 62 placements over 58 books — genres are hand-assigned, so this is a
    // sample of the 1,510, and the card has to say so.
    coverage: { total: 62, books: 58 },
  },
};

test("the library-composition card states each dimension with its coverage", async ({
  page,
}) => {
  // Route-mocked: the real mix depends on what the shared fixture library
  // holds, which no spec can pin without mutating it for every other one.
  await page.route("**/api/rpc/library-composition", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(COMPOSITION),
    }),
  );
  await gotoReady(page, "/stats");
  await openLibraryScope(page);

  const card = page.getByTestId("stats-library-composition");
  await expect(card).toBeVisible();
  // Each panel titles itself; the scope switch above them already says these
  // are the shelf's figures, so a section heading would be a third statement
  // of the same thing.
  for (const title of ["Formats", "Languages", "Publishers", "Genres"]) {
    await expect(card).toContainText(title);
  }
  await expect(card.getByTestId("stats-composition-formats")).toContainText(
    "EPUB",
  );
  await expect(card.getByTestId("stats-composition-decades")).toContainText(
    "1990s",
  );
  // The genre coverage is the whole point: genres are hand-assigned, so the
  // slices describe 58 books, not 1,510.
  await expect(card.getByTestId("stats-composition-genres")).toContainText(
    "hand-assigned \u2014 across 58 of 1,510 books",
  );
  // Dual-format books are counted in both bars, and said so.
  await expect(card.getByTestId("stats-composition-formats")).toContainText(
    "+70 books held in more than one format",
  );
  // Ghosted rows are named rather than left to make the bars not add up.
  await expect(page.getByTestId("stats-composition-ghosted")).toContainText(
    "4 books excluded",
  );
});

test("a composition dimension with no data renders an empty state, not an empty chart", async ({
  page,
}) => {
  await page.route("**/api/rpc/library-composition", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(COMPOSITION),
    }),
  );
  await gotoReady(page, "/stats");
  await openLibraryScope(page);

  const publishers = page.getByTestId("stats-composition-publishers");
  await expect(publishers).toContainText("No publisher metadata yet.");
  // An axis with no bars on it is the failure this replaces.
  await expect(publishers.getByTestId("stats-composition-bar")).toHaveCount(0);
});

test("a library with nothing to describe renders no composition card at all", async ({
  page,
}) => {
  await page.route("**/api/rpc/library-composition", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ books: 0 }),
    }),
  );
  // The card is also absent when the fetch never happened, so waiting on the
  // mocked response is what makes this test fail if the route moves rather
  // than pass for the wrong reason.
  const answered = page.waitForResponse("**/api/rpc/library-composition");
  await gotoReady(page, "/stats");
  await answered;
  await openLibraryScope(page);

  await expect(page.getByTestId("stats-scope-library")).toBeVisible();
  await expect(page.getByTestId("stats-library-composition")).toHaveCount(0);
});

test("the standing band and the hero do not change with the switcher", async ({
  page,
}) => {
  // The load-bearing contract: the pills govern the windowed band and nothing
  // else. A regression here is the whole redesign undone.
  await gotoReady(page, "/stats");
  const standing = page.getByTestId("stats-alltime-section");
  const hero = page.getByTestId("stats-hero");
  await expect(standing).toBeVisible();
  const standingBefore = await standing.textContent();
  const heroBefore = await hero.textContent();

  await selectPeriod(page, "year");

  await expect(page.getByTestId("stats-window-label")).toContainText(
    "year to date",
  );
  await expect.poll(() => standing.textContent()).toBe(standingBefore);
  await expect.poll(() => hero.textContent()).toBe(heroBefore);
});

test("a user with no activity sees the friendly empty state", async ({
  page,
}) => {
  // Forcing an all-zero summary through the rpc route is the deterministic
  // stand-in for a fresh user — self-registration is disabled after the first
  // account, and the shared test user has seeded sessions.
  const emptySummary = {
    range: "month",
    reading_seconds: 0,
    listening_seconds: 0,
    sessions: 0,
    active_days: 0,
    longest_streak_days: 0,
    busiest_week_start: null,
    busiest_week_seconds: 0,
    books_finished: 0,
    heatmap: [],
    top_authors: [],
    top_tags: [],
    finished_books: [],
  };
  await page.route("**/api/rpc/stats", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(emptySummary),
    }),
  );

  await gotoReady(page, "/stats");
  await expect(page.getByTestId("stats-empty")).toBeVisible();
  await expect(page.getByText("No reading activity yet")).toBeVisible();
  await expect(page.getByTestId("stats-period-section")).toHaveCount(0);
  await expect(page.getByTestId("stats-alltime-section")).toHaveCount(0);
  // The hero stays: the goals are set from it, and a reader with no activity
  // is exactly who wants to set one.
  await expect(page.getByTestId("stats-hero")).toBeVisible();
});

/**
 * A summary carrying whatever time-pattern fields a test wants to pin,
 * over the same minimal scaffold the other route-mocked tests here use.
 */
function summaryWith(extra: Record<string, unknown>) {
  return {
    range: "month",
    reading_seconds: 18720,
    listening_seconds: 0,
    sessions: 2,
    active_days: 2,
    longest_streak_days: 1,
    current_streak_days: 1,
    busiest_week_start: null,
    busiest_week_seconds: 18720,
    books_finished: 1,
    heatmap: [],
    top_authors: [],
    top_tags: [],
    finished_books: [],
    ...extra,
  };
}

test("the reading clock draws every local hour and names the peak", async ({
  page,
}) => {
  // Route-mocked so the buckets are pinned: the suite shares one server and
  // one user, and any other spec posting a session moves these ticks.
  const hours = Array.from({ length: 24 }, (_, hour) => ({
    hour,
    seconds: hour === 21 ? 15120 : hour === 9 ? 3600 : 0,
  }));
  const days = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"].map(
    (label, weekday) => ({
      weekday,
      label,
      seconds: label === "Sun" ? 15120 : 0,
    }),
  );
  await page.route("**/api/rpc/stats", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(
        summaryWith({
          hour_of_day: hours,
          day_of_week: days,
          unzoned_seconds: 15120,
        }),
      ),
    }),
  );

  await gotoReady(page, "/stats");

  // All 24 ticks render, quiet hours included — the shape of a day is the
  // information, so an omitted hour would misdescribe it.
  const dial = page.getByTestId("stats-clock-dial");
  await expect(dial).toBeVisible();
  await expect(dial.locator(".st-clock-tick")).toHaveCount(24);
  // The peak is derived, and reads as a clock time rather than an index. An
  // all-zero window has none, which the next test pins.
  await expect(page.getByTestId("stats-clock-peak")).toHaveText("9pm");
  // The sentence beside the dial names the part of the day, not just a number.
  // 9pm is the last hour of the evening band, so this window is an evening
  // one — the late band starts at 10.
  await expect(page.getByTestId("stats-clock-line")).toContainText(
    "Evenings are yours",
  );

  // Weekdays are the server's own labels in the server's own order — the web
  // never decides where the week starts.
  const dayStrip = page.getByTestId("stats-when-weekdays");
  await expect
    .poll(() => dayStrip.locator(".st-day-label").allInnerTexts())
    .toEqual(["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]);
  // A day with nothing on it reads as an absence, never as a measured zero.
  await expect
    .poll(() => dayStrip.locator(".st-day-readout").allInnerTexts())
    .toEqual([
      "\u2014",
      "\u2014",
      "\u2014",
      "\u2014",
      "\u2014",
      "\u2014",
      "4h 12m",
    ]);

  // Activity the server couldn't place on a local clock is disclosed, not
  // folded into a UTC hour.
  await expect(page.getByTestId("stats-when-unzoned")).toContainText("4h 12m");
});

test("the reading clock says so rather than drawing an empty dial", async ({
  page,
}) => {
  // A window whose sessions all predate capture-time timezones: the strips are
  // zero-filled to a fixed width, so drawing them would show 24 empty columns
  // that look like a measured day.
  await page.route("**/api/rpc/stats", (route) =>
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(
        summaryWith({
          hour_of_day: Array.from({ length: 24 }, (_, hour) => ({
            hour,
            seconds: 0,
          })),
          // Zero-filled to 7 like the server always sends it — the card's
          // empty state keys on the hour strip, not on a missing weekday
          // array, so the mock has no business being a shape the server
          // can't produce.
          day_of_week: ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"].map(
            (label, weekday) => ({ weekday, label, seconds: 0 }),
          ),
          unzoned_seconds: 18720,
        }),
      ),
    }),
  );

  await gotoReady(page, "/stats");

  await expect(page.getByTestId("stats-when-empty")).toBeVisible();
  await expect(page.getByTestId("stats-clock-dial")).toHaveCount(0);
  // No peak hour is an em-dash, never midnight: the first maximum of a row of
  // zeros is index 0, and reporting it would invent a habit.
  await expect(page.getByTestId("stats-clock-peak")).toHaveCount(0);
  await expect(page.getByTestId("stats-when-unzoned")).toContainText("5h 12m");
});

/**
 * Reset every goal so the read-only assertions below start from a known
 * state. The REST writes invalidate this user's stats cache, so the page reads
 * the cleared values immediately rather than after the TTL.
 */
async function clearAllGoals(request: APIRequestContext) {
  const annual = await request.put("/api/stats/goal", {
    data: { target: null },
  });
  expect(annual.status(), "clearing the reading goal failed").toBe(200);
  for (const kind of ["pages", "minutes"]) {
    const resp = await request.put("/api/stats/goal/daily", {
      data: { kind, target: null },
    });
    expect(resp.status(), `clearing the daily ${kind} goal failed`).toBe(200);
  }
}

test("the stats page reports goals but never edits them", async ({
  page,
  request,
}) => {
  // Goals are account configuration and all three are set together in
  // Settings → Account. The page that *reports* them must not grow an editor
  // back: that split is the whole reason there is one Edit button.
  await request.put("/api/stats/goal", { data: { target: 24 } });
  await request.put("/api/stats/goal/daily", {
    data: { kind: "pages", target: 30 },
  });
  await gotoReady(page, "/stats");

  await expect(page.getByTestId("stats-goal-figure")).toContainText(
    /of 24 books/,
  );
  await expect(page.getByTestId("stats-goal-progress")).toHaveAttribute(
    "aria-valuemax",
    "24",
  );
  await expect(page.getByTestId("stats-daily-pages-figure")).toContainText(
    /of 30 pages/,
  );
  // The untargeted kind still reports today's figure — a readout, not a goal —
  // and carries its own timeframe, since the card header has moved to "Every
  // day" for the targeted row's sake.
  await expect(page.getByTestId("stats-daily-minutes-today")).toBeVisible();
  await expect(page.getByTestId("stats-daily-goals")).toContainText(
    "Minutes today",
  );

  for (const gone of [
    "stats-goal-edit",
    "stats-goal-input",
    "stats-goal-save",
    "stats-daily-pages-edit",
    "stats-daily-minutes-edit",
  ]) {
    await expect(page.getByTestId(gone)).toHaveCount(0);
  }
});

test("with no goals the hero reports the real figures and links to the editor", async ({
  page,
  request,
}) => {
  await clearAllGoals(request);
  await gotoReady(page, "/stats");

  // No target means no ring and no bar — both are claims a target makes — but
  // the figures behind them are still worth showing, so the reader can see
  // where they stand before committing to anything.
  await expect(page.getByTestId("stats-goal-progress")).toHaveCount(0);
  await expect(page.getByTestId("stats-goal-year-to-date")).toBeVisible();
  await expect(page.getByTestId("stats-daily-pages-today")).toBeVisible();
  await expect(page.getByTestId("stats-daily-minutes-today")).toBeVisible();
  await expect(page.getByTestId("stats-daily-pages-progress")).toHaveCount(0);

  // Each label says its timeframe exactly once: the annual kicker carries the
  // year, and the daily card's header carries "Today" so its rows don't.
  const hero = page.getByTestId("stats-hero");
  await expect(hero).toContainText(/\d{4} so far/);
  await expect(hero).not.toContainText("This year");
  await expect(page.getByTestId("stats-daily-goals")).not.toContainText(
    "Pages a day",
  );

  // Both halves link to the one place goals are set.
  await expect(page.getByTestId("stats-daily-set-link")).toBeVisible();
  await page.getByTestId("stats-goal-set-link").click();
  await expect(page).toHaveURL(/\/settings/);
  await expect(page.getByTestId("account-goals-card")).toBeVisible();
});

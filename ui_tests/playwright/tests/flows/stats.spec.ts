// Reading-stats page (/stats): layout, the Week / Month / Year / Lifetime
// dropdown embedded in the page title, the all-time section's immunity to
// period changes, and the zero-activity empty state.
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

/** Open the in-title period dropdown and return its dialog locator. */
async function openPeriodMenu(page: Page) {
  await page.getByTestId("stats-range-trigger").click();
  const menu = page.getByRole("dialog", { name: "Period" });
  await expect(menu).toBeVisible();
  return menu;
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

  // Title carries the default period word (Month is the default range).
  await expect(
    page.getByRole("heading", { name: "Your reading month" }),
  ).toBeVisible();

  // The title's period word opens the dropdown, which offers all four
  // periods with Month pre-selected.
  const menu = await openPeriodMenu(page);
  for (const label of ["Week", "Month", "Year", "Lifetime"]) {
    await expect(menu.getByRole("button", { name: label })).toBeVisible();
  }
  await expect(menu.getByRole("button", { name: "Month" })).toHaveAttribute(
    "aria-pressed",
    "true",
  );

  // The scrim dismisses it without changing the period.
  await page.getByTestId("stats-range-scrim").click();
  await expect(menu).toHaveCount(0);
  await expect(
    page.getByRole("heading", { name: "Your reading month" }),
  ).toBeVisible();

  // ESC dismisses it too (the menu takes focus on mount), also without
  // changing the period.
  const reopened = await openPeriodMenu(page);
  await reopened.press("Escape");
  await expect(reopened).toHaveCount(0);
  await expect(
    page.getByRole("heading", { name: "Your reading month" }),
  ).toBeVisible();

  // Period-scoped section above, explicitly divided all-time section below.
  await expect(page.getByTestId("stats-period-section")).toBeVisible();
  // The time-pattern card renders in both its states (strips or the
  // no-local-time note), so its presence is structural.
  await expect(page.getByTestId("stats-when")).toBeVisible();
  // The strips are the live end of the whole local-time path — the seeded
  // reports' `utc_offset_minutes` reaching the session column, the shifted
  // rollup, and the wire fields. Every other assertion in this file mocks
  // `/api/rpc/stats`, so without this one a serde rename or a dropped bind
  // would leave the suite green with the card stuck in its empty state.
  await expect(page.getByTestId("stats-when-empty")).toHaveCount(0);
  await expect(page.getByTestId("stats-when-hours")).toBeVisible();
  await expect(page.getByTestId("stats-when-weekdays")).toBeVisible();
  await expect(page.getByText("Not tied to the period above.")).toBeVisible();
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
  await expect(
    page.getByRole("heading", { name: "Your reading month" }),
  ).toBeVisible();
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

test("switching the period re-queries and updates the period section", async ({
  page,
}) => {
  await gotoReady(page, "/stats");
  const menu = await openPeriodMenu(page);

  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/stats",
      expectedBody: { range: "week" },
      expectedStatus: 200,
    },
    async () => menu.getByRole("button", { name: "Week" }).click(),
  );

  // Picking a period closes the menu and rewrites the title.
  await expect(menu).toHaveCount(0);
  await expect(
    page.getByRole("heading", { name: "Your reading week" }),
  ).toBeVisible();

  // Reopening shows the new selection.
  const reopened = await openPeriodMenu(page);
  await expect(reopened.getByRole("button", { name: "Week" })).toHaveAttribute(
    "aria-pressed",
    "true",
  );
  await expect(reopened.getByRole("button", { name: "Month" })).toHaveAttribute(
    "aria-pressed",
    "false",
  );
});

test("the heatmap and genre donut render from seeded activity", async ({
  page,
}) => {
  await gotoReady(page, "/stats");

  // Heatmap: trailing-year grid with both streak figures in the card header
  // and at least one active cell — keyed off the cell tooltip ("… on
  // YYYY-MM-DD", which only active cells carry) rather than the intensity CSS
  // classes.
  const heatmap = page.getByTestId("stats-heatmap");
  await expect(heatmap).toBeVisible();
  // Both figures are asserted by label, not by value: the shared fixture's
  // sessions are seeded at fixed timestamps, so whether a run is live depends
  // on when the suite runs.
  await expect(page.getByTestId("stats-current-streak")).toContainText(
    "Current streak",
  );
  await expect(page.getByTestId("stats-longest-streak")).toContainText(
    "Longest streak",
  );
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

  // Format split: both seeded formats appear with percentages.
  const split = page.getByTestId("stats-format-split");
  await expect(split).toContainText("Read");
  await expect(split).toContainText("Listened");

  // Length split: the card is present either way. Which bars it draws depends
  // on what the shared fixture user has finished this month, so this asserts
  // only that the surface renders one of its two states.
  await expect(page.getByTestId("stats-length-split")).toBeVisible();
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
  await expect(bars.last()).toHaveClass(/st-mo-current/);
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

  const card = page.getByTestId("stats-library-size");
  await expect(card).toBeVisible();
  await expect(card).toContainText("412M");
  await expect(card).toContainText("1.6M");
  // Never a bare total: the denominator is what makes the number a fact.
  await expect(card).toContainText("across 1,204 of 1,510 books");
  await expect(card.getByTestId("stats-library-figure")).toHaveCount(2);

  // It sits in the all-time section, so it must not move with the switcher.
  const before = await card.textContent();
  const menu = await openPeriodMenu(page);
  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/stats",
      expectedBody: { range: "week" },
      expectedStatus: 200,
    },
    async () => menu.getByRole("button", { name: "Week" }).click(),
  );
  await expect(
    page.getByRole("heading", { name: "Your reading week" }),
  ).toBeVisible();
  await expect.poll(() => card.textContent()).toBe(before);
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

  // Three zeroes would read as a claim about the collection rather than about
  // the backfill.
  await expect(page.getByTestId("stats-alltime-section")).toBeVisible();
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

  const card = page.getByTestId("stats-library-composition");
  await expect(card).toBeVisible();
  // Named apart from the period-scoped "How you consumed them" split, which
  // is read-vs-listened seconds rather than the shelf's own format mix.
  await expect(card).toContainText("What your library is made of");
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
  await expect(card.getByTestId("stats-composition-ghosted")).toContainText(
    "4 books excluded",
  );

  // It sits in the all-time section, so it must not move with the switcher.
  const before = await card.textContent();
  const menu = await openPeriodMenu(page);
  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/stats",
      expectedBody: { range: "week" },
      expectedStatus: 200,
    },
    async () => menu.getByRole("button", { name: "Week" }).click(),
  );
  await expect(
    page.getByRole("heading", { name: "Your reading week" }),
  ).toBeVisible();
  await expect.poll(() => card.textContent()).toBe(before);
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

  await expect(page.getByTestId("stats-alltime-section")).toBeVisible();
  await expect(page.getByTestId("stats-library-composition")).toHaveCount(0);
});

test("the all-time section does not change with the switcher", async ({
  page,
}) => {
  await gotoReady(page, "/stats");
  const allTime = page.getByTestId("stats-heatmap");
  await expect(allTime).toBeVisible();
  const before = await allTime.textContent();

  const menu = await openPeriodMenu(page);
  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/stats",
      expectedBody: { range: "year" },
      expectedStatus: 200,
    },
    async () => menu.getByRole("button", { name: "Year" }).click(),
  );

  await expect(
    page.getByRole("heading", { name: "Your reading year" }),
  ).toBeVisible();
  await expect.poll(() => allTime.textContent()).toBe(before);
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

test("the time-pattern card charts every local hour and weekday", async ({
  page,
}) => {
  // Route-mocked so the buckets are pinned: the suite shares one server and
  // one user, and any other spec posting a session moves these bars.
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

  // All 24 columns render, quiet hours included — the shape of a day is the
  // information, so an omitted hour would misdescribe it. Every third one is
  // labelled; the rest keep their place with a blank label.
  const hourStrip = page.getByTestId("stats-when-hours");
  await expect(hourStrip).toBeVisible();
  const hourLabels = hourStrip.getByTestId("stats-when-col-label");
  await expect(hourLabels).toHaveCount(24);
  await expect
    .poll(() => hourLabels.allInnerTexts().then((t) => t.filter(Boolean)))
    .toEqual(["00", "03", "06", "09", "12", "15", "18", "21"]);
  // The magnitude nobody can read off a bar rides the hover title, as a
  // clock time rather than a bare number.
  await expect(hourStrip.locator('[title="21:00 · 4h 12m"]')).toBeVisible();

  // Weekdays are the server's own labels in the server's own order — the web
  // never decides where the week starts.
  const dayStrip = page.getByTestId("stats-when-weekdays");
  await expect
    .poll(() => dayStrip.getByTestId("stats-when-col-label").allInnerTexts())
    .toEqual(["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"]);
  await expect(dayStrip.locator('[title="Sun · 4h 12m"]')).toBeVisible();

  // Activity the server couldn't place on a local clock is disclosed, not
  // folded into a UTC hour.
  await expect(page.getByTestId("stats-when-unzoned")).toContainText("4h 12m");
});

test("the time-pattern card says so rather than drawing flat bars", async ({
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
  await expect(page.getByTestId("stats-when-hours")).toHaveCount(0);
  await expect(page.getByTestId("stats-when-unzoned")).toContainText("5h 12m");
});

/**
 * Reset the shared user's annual goal so the goal tests start from the
 * invitation state. The REST write invalidates that user's stats cache, so the
 * page reads the cleared value immediately rather than after the TTL.
 */
async function clearReadingGoal(request: APIRequestContext) {
  const resp = await request.put("/api/stats/goal", {
    data: { target: null },
  });
  expect(resp.status(), "clearing the reading goal failed").toBe(200);
}

test("the annual goal band invites, saves, persists, and clears", async ({
  page,
  request,
}) => {
  await clearReadingGoal(request);
  await gotoReady(page, "/stats");

  // AC4: no goal is an invitation, never a zero-of-zero bar.
  const band = page.getByTestId("stats-goal");
  await expect(band).toBeVisible();
  await expect(page.getByTestId("stats-goal-invite")).toBeVisible();
  await expect(page.getByTestId("stats-goal-progress")).toHaveCount(0);

  await page.getByTestId("stats-goal-edit").click();
  await page.getByTestId("stats-goal-input").fill("24");
  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/stats-goal",
      expectedBody: { update: { target: 24 } },
      expectedStatus: 200,
    },
    async () => page.getByTestId("stats-goal-save").click(),
  );

  // AC5: the saved target is on screen straight away, not after the 60s
  // stats cache TTL.
  await expect(page.getByTestId("stats-goal-figure")).toContainText(
    /of 24 books/,
  );
  await expect(page.getByTestId("stats-goal-progress")).toHaveAttribute(
    "aria-valuemax",
    "24",
  );

  // AC1: it survives a fresh page load.
  await gotoReady(page, "/stats");
  await expect(page.getByTestId("stats-goal-figure")).toContainText(
    /of 24 books/,
  );

  // AC3: the band is annual, so the period switcher must not move it.
  const before = await band.textContent();
  const menu = await openPeriodMenu(page);
  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/stats",
      expectedBody: { range: "week" },
      expectedStatus: 200,
    },
    async () => menu.getByRole("button", { name: "Week" }).click(),
  );
  await expect(
    page.getByRole("heading", { name: "Your reading week" }),
  ).toBeVisible();
  await expect.poll(() => band.textContent()).toBe(before);

  // Clearing drops the row rather than storing a zero target, so the
  // invitation comes back.
  await page.getByTestId("stats-goal-edit").click();
  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/stats-goal",
      expectedBody: { update: {} },
      expectedStatus: 200,
    },
    async () => page.getByTestId("stats-goal-clear").click(),
  );
  await expect(page.getByTestId("stats-goal-invite")).toBeVisible();
  await expect(page.getByTestId("stats-goal-progress")).toHaveCount(0);
});

test("a failed goal save surfaces the error and leaves the goal unset", async ({
  page,
  request,
}) => {
  await clearReadingGoal(request);
  await page.route("**/api/rpc/stats-goal", (route) =>
    route.fulfill({
      status: 500,
      contentType: "text/plain",
      body: "goal write failed",
    }),
  );

  await gotoReady(page, "/stats");
  await page.getByTestId("stats-goal-edit").click();
  await page.getByTestId("stats-goal-input").fill("12");
  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/stats-goal",
      expectedBody: { update: { target: 12 } },
      expectedStatus: 500,
    },
    async () => page.getByTestId("stats-goal-save").click(),
  );

  await expect(page.getByTestId("stats-goal-error")).toBeVisible();
  // The band stays in its pre-save state — no optimistic bar for a write the
  // server rejected.
  await expect(page.getByTestId("stats-goal-invite")).toBeVisible();
  await expect(page.getByTestId("stats-goal-progress")).toHaveCount(0);
});

test("the session log lists stitched sittings under the aggregates", async ({
  page,
}) => {
  await gotoReady(page, "/stats");

  const log = page.getByTestId("session-log");
  await expect(log).toBeVisible();
  await expect(log.getByRole("heading", { name: "Session log" })).toBeVisible();

  // The list arrives from its own post-mount fetch, so poll rather than
  // asserting against the first paint.
  const rows = page.getByTestId("session-log-row");
  await expect.poll(() => rows.count()).toBeGreaterThan(0);

  // AC2 — book, start time, format, and length on every row. Asserted by
  // shape: other specs share this user and add sittings of their own, and the
  // arithmetic is pinned by the db::stats unit tests.
  const first = rows.first();
  await expect(first).toContainText(/\d{4}/); // the year in the start stamp
  await expect(first).toContainText(/Read|Listened/);
  await expect(first).toContainText(/\d+(h|m)/);

  // AC3 (the stitch itself) is pinned by the db::stats::sessions unit tests,
  // not here: this is the user-wide log for a user every other spec also
  // records sittings against, so any one seeded sitting can be pushed off the
  // newest page by a parallel spec and must not be asserted by identity.
  await expect(page.getByTestId("session-log-empty")).toHaveCount(0);
});

test("a failed session-log fetch surfaces an error without blanking the page", async ({
  page,
}) => {
  await page.route("**/api/rpc/stats/sessions", (route) =>
    route.fulfill({ status: 500, body: "boom" }),
  );

  await gotoReady(page, "/stats");
  await expect(page.getByTestId("session-log-error")).toBeVisible();
  await expect(page.getByTestId("session-log-row")).toHaveCount(0);
  // The aggregates above it are unaffected — the log is its own read.
  await expect(page.getByTestId("stats-alltime-section")).toBeVisible();
});

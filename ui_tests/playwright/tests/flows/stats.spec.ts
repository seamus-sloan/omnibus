// Reading-stats page (/stats): layout, the Week / Month / Year / Lifetime
// dropdown embedded in the page title, the all-time section's immunity to
// period changes, and the zero-activity empty state.
//
// Sessions are seeded once in beforeAll — before the first /stats visit — so
// the server-side 60s stats cache never captures an empty summary for the
// shared test user. The session recorder skips uuids that don't resolve to an
// indexed book, so the library is seeded first and sessions ride a real
// fixture uuid.

import type { Page } from "@playwright/test";
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
  // #1029: the finished journal seeded in beforeAll gives the Pages tile a
  // real (spine-word-count-derived) estimate — a digit, not the em-dash
  // placeholder. The exact count depends on fixture text length, covered by
  // db::stats unit tests instead of pinned here.
  await expect(page.getByTestId("stats-tile-pages")).toContainText(/\d/);
  await expect(page.getByTestId("stats-tile-listening")).toContainText(
    /\d+\s*(m|h)/,
  );
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

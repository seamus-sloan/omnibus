// Reading-stats page (/stats): layout, the Week / Month / Year / Lifetime
// period switcher, the all-time section's immunity to switcher changes, and
// the zero-activity empty state.
//
// Sessions are seeded once in beforeAll — before the first /stats visit — so
// the server-side 60s stats cache never captures an empty summary for the
// shared test user. The session recorder skips uuids that don't resolve to an
// indexed book, so the library is seeded first and sessions ride a real
// fixture uuid.
import { expect, test } from "../fixtures/test";
import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { expectMutation } from "../utils/api";
import { fetchBookUuidByTitle } from "../utils/ebooks";
import { expectNavVisible, gotoReady } from "../utils/nav";
import { fixturesDir, seedLibrary } from "../utils/seed";

// Serial: the beforeAll seed mutates shared server state (library reindex +
// session inserts). Under the suite's fullyParallel default it would re-run
// per worker and the concurrent writes 500 on SQLite's write lock.
test.describe.configure({ mode: "serial" });

/** Late-2023 anchor: inside Lifetime, outside the week/month/year windows. */
const OLD_SESSION_AT = 1_700_000_000;

test.beforeAll(async ({ request }) => {
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
  const uuid = await fetchBookUuidByTitle(request, FIXTURE_BOOKS[0].title);

  const now = Math.floor(Date.now() / 1000);
  const session = (startedAt: number, secs: number, format: "epub" | "audio") => ({
    book_uuid: uuid,
    format,
    started_at: startedAt,
    ended_at: startedAt + secs,
    progress_units: secs,
  });
  const resp = await request.post("/api/progress/sessions", {
    data: [
      session(now - 60, 600, "epub"),
      session(OLD_SESSION_AT, 900, "epub"),
      session(now - 120, 900, "audio"),
    ],
  });
  expect(resp.status(), "seeding stats sessions failed").toBe(200);
  const body = (await resp.json()) as { recorded: number };
  expect(body.recorded, "session seeds were skipped").toBe(3);

  // Feed the headline tiles: a star rating and a finished (100%) journal.
  const rating = await request.post("/api/ratings", {
    data: { book_uuid: uuid, stars: 4.5 },
  });
  expect(rating.status(), "seeding a rating failed").toBe(200);
  const journal = await request.post("/api/journals", {
    data: { book_uuid: uuid, body_md: "Finished it — stats spec seed.", progress: 100 },
  });
  expect(journal.status(), "seeding a finished journal failed").toBe(200);
});

test("renders the stats page layout", async ({ page }) => {
  await gotoReady(page, "/");
  await expectNavVisible(page);
  await page.getByRole("navigation", { name: "Primary" }).getByRole("link", { name: "Stats" }).click();
  await expect(page).toHaveURL(/\/stats$/);
  await page.waitForLoadState("networkidle");

  // Title carries the default period word (Month is the default range).
  await expect(page.getByRole("heading", { name: "Your reading month" })).toBeVisible();

  // The switcher offers all four periods with Month pre-selected.
  const seg = page.getByRole("group", { name: "Period" });
  for (const label of ["Week", "Month", "Year", "Lifetime"]) {
    await expect(seg.getByRole("button", { name: label })).toBeVisible();
  }
  await expect(seg.getByRole("button", { name: "Month" })).toHaveAttribute("aria-pressed", "true");

  // Period-scoped section above, explicitly divided all-time section below.
  await expect(page.getByTestId("stats-period-section")).toBeVisible();
  await expect(page.getByText("Not tied to the period above.")).toBeVisible();
  await expect(page.getByTestId("stats-alltime-section")).toBeVisible();
});

test("headline tiles render finished, avg rating, pages, and listening", async ({ page }) => {
  await gotoReady(page, "/stats");

  // Values are asserted by shape, not exact numbers — other specs sharing the
  // test user can add sessions/ratings/journals; the arithmetic is covered by
  // the db::stats unit tests.
  await expect(page.getByTestId("stats-tile-finished")).toContainText(/\d/);
  await expect(page.getByTestId("stats-tile-finished")).toContainText("Finished");
  await expect(page.getByTestId("stats-tile-avg-rating")).toContainText(/\d\.\d\s*★/);
  await expect(page.getByTestId("stats-tile-pages")).toContainText("—");
  await expect(page.getByTestId("stats-tile-listening")).toContainText(/\d+\s*(m|h)/);
});

test("switching the period re-queries and updates the period section", async ({ page }) => {
  await gotoReady(page, "/stats");
  const seg = page.getByRole("group", { name: "Period" });

  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/stats",
      expectedBody: { range: "week" },
      expectedStatus: 200,
    },
    async () => seg.getByRole("button", { name: "Week" }).click(),
  );

  await expect(seg.getByRole("button", { name: "Week" })).toHaveAttribute("aria-pressed", "true");
  await expect(seg.getByRole("button", { name: "Month" })).toHaveAttribute("aria-pressed", "false");
  await expect(page.getByRole("heading", { name: "Your reading week" })).toBeVisible();
});

test("the all-time section does not change with the switcher", async ({ page }) => {
  await gotoReady(page, "/stats");
  const allTime = page.getByTestId("stats-alltime-summary");
  await expect(allTime).toBeVisible();
  const before = await allTime.textContent();

  const seg = page.getByRole("group", { name: "Period" });
  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/stats",
      expectedBody: { range: "year" },
      expectedStatus: 200,
    },
    async () => seg.getByRole("button", { name: "Year" }).click(),
  );

  await expect(page.getByRole("heading", { name: "Your reading year" })).toBeVisible();
  await expect.poll(() => allTime.textContent()).toBe(before);
});

test("a user with no activity sees the friendly empty state", async ({ page }) => {
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

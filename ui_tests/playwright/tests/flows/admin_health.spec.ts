// The admin server-health page (#952) and its live-update poll loop (#955).
// `AdminHealthPage` fires a POST /api/rpc/admin-health on mount and again
// every POLL_INTERVAL_MS (5s, `frontend/src/pages/admin_health.rs`) from a
// post-mount `use_future`, with no other UI trigger on the page (unlike the
// worker-status indicator, there's no button here).
//
// The layout test hits the real endpoint (the seeded admin session already
// has access) so it proves the real end-to-end fetch + render, the same way
// `logs.spec.ts`'s layout test reads real on-disk logs.
//
// The live-update test mocks the endpoint — the same technique
// `worker-status.spec.ts` uses for its own 1 Hz poll loop — because the
// values under assertion (book counts, ghosted-book warning, worker queue
// rows) need to be deterministic and the change needs to be attributable to
// a *second* poll tick, not to real library/worker state a parallel spec
// might also be touching. We arm `waitForResponse` for each poll rather than
// sleeping a fixed duration, per rule 04.

import type { Locator, Page } from "@playwright/test";
import { expect, test } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import { expectNavVisible, gotoReady } from "../utils/nav";

const ADMIN_HEALTH = "/admin/health";
const ADMIN_HEALTH_RPC = "**/api/rpc/admin-health";

interface AdminHealthReport {
  index: {
    ebook_library_path: string | null;
    audiobook_library_path: string | null;
    book_count: number;
    author_count: number;
    series_count: number;
    ghosted_book_count: number;
  };
  worker_queue: Array<{
    kind: string;
    queue_depth: number;
    recent_completions_ms: number[];
  }>;
  fts: { books_rows: number; fts_rows: number; in_sync: boolean };
  storage: {
    library_bytes: number;
    covers_bytes: number;
    thumbs_bytes: number;
    thumbs_cap_bytes: number;
    hls_bytes: number;
    hls_cap_bytes: number;
    export_epub_bytes: number;
    export_epub_cap_bytes: number;
  };
  last_errors: Array<{
    timestamp: number;
    target: string;
    message: string;
    book_uuid?: string | null;
  }>;
}

const BASE_STORAGE: AdminHealthReport["storage"] = {
  library_bytes: 1_000_000,
  covers_bytes: 200_000,
  thumbs_bytes: 50_000,
  thumbs_cap_bytes: 1_000_000_000,
  hls_bytes: 0,
  hls_cap_bytes: 5_000_000_000,
  export_epub_bytes: 0,
  export_epub_cap_bytes: 5_000_000_000,
};

/**
 * Install a route handler that returns the same canned `AdminHealthReport`
 * on every poll. Returns a setter so the test can flip the "server-side"
 * report mid-flow without re-installing the route — mirrors
 * `mockWorkerStatus` in `worker-status.spec.ts`.
 */
async function mockAdminHealth(
  page: Page,
  initial: AdminHealthReport,
): Promise<(next: AdminHealthReport) => void> {
  let current = initial;
  await page.route(ADMIN_HEALTH_RPC, async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(current),
    });
  });
  return (next: AdminHealthReport) => {
    current = next;
  };
}

/** Wait for the next poll response to land (armed *before* triggering it). */
function waitForAdminHealthPoll(page: Page) {
  return page.waitForResponse(
    (r) =>
      r.request().method() === "POST" &&
      r.url().includes("/api/rpc/admin-health") &&
      r.status() === 200,
  );
}

/**
 * `AhKvRow` (`frontend/src/pages/admin_health.rs`) doesn't carry a per-row
 * testid — its label/value pair is the only way to pick out one stat, so a
 * `locator()` filter on its class is the correct last resort per rule 04.
 */
function kvValue(card: Locator, label: string): Locator {
  return card.locator(".ah-kv-row", { hasText: label }).locator(".ah-kv-value");
}

test("renders the admin health layout", async ({ page }) => {
  // `should_poll` re-reads `is_admin()` every tick, and `CurrentUser` boots
  // unresolved (`Signal::new(None)` in `lib.rs`) until its own async fetch
  // lands — so the poll loop's very first tick almost always sees
  // `is_admin() == false` and skips straight to its 5s sleep, landing the
  // *real* first admin-health request a full POLL_INTERVAL_MS after mount
  // rather than immediately. `expectMutation`'s default 5s timeout races
  // that, so give this one enough room for one skipped tick plus WASM boot
  // overhead.
  await expectMutation(
    page,
    {
      method: "POST",
      url: "/api/rpc/admin-health",
      expectedStatus: 200,
      timeout: 12_000,
    },
    async () => gotoReady(page, ADMIN_HEALTH),
  );

  await expect(
    page.getByRole("heading", { level: 1, name: "Server health" }),
  ).toBeVisible();
  await expect(page.getByTestId("admin-health-grid")).toBeVisible();

  const indexCard = page.getByTestId("admin-health-index-card");
  await expect(
    indexCard.getByRole("heading", { name: "Index status" }),
  ).toBeVisible();
  await expect(indexCard).toContainText("Books indexed");
  await expect(indexCard).toContainText("Authors indexed");
  await expect(indexCard).toContainText("Series indexed");

  const workerCard = page.getByTestId("admin-health-worker-card");
  await expect(
    workerCard.getByRole("heading", { name: "Worker queue" }),
  ).toBeVisible();
  // Shared dev server — a parallel spec's scan may or may not be running,
  // so assert the section rendered one of its two valid states rather than
  // a specific one.
  await expect(
    page
      .getByTestId("admin-health-worker-empty")
      .or(page.getByTestId("admin-health-worker-table")),
  ).toBeVisible();

  const ftsCard = page.getByTestId("admin-health-fts-card");
  await expect(
    ftsCard.getByRole("heading", { name: "Search index (FTS)" }),
  ).toBeVisible();
  await expect(ftsCard).toContainText("Books rows");
  await expect(ftsCard).toContainText("FTS rows");

  const storageCard = page.getByTestId("admin-health-storage-card");
  await expect(
    storageCard.getByRole("heading", { name: "Storage" }),
  ).toBeVisible();
  await expect(storageCard).toContainText("Library content");
  await expect(storageCard).toContainText("Thumbnail cache");

  const errorsCard = page.getByTestId("admin-health-errors-card");
  await expect(
    errorsCard.getByRole("heading", { name: "Last errors" }),
  ).toBeVisible();
  await expect(
    page
      .getByTestId("admin-health-errors-empty")
      .or(page.getByTestId("admin-health-errors-table")),
  ).toBeVisible();

  await expectNavVisible(page);
});

test("live-updates index stats and the worker queue after a poll tick, without navigation (AC1 of #1851)", async ({
  page,
}) => {
  const initial: AdminHealthReport = {
    index: {
      ebook_library_path: "/data/ebooks",
      audiobook_library_path: null,
      book_count: 12,
      author_count: 4,
      series_count: 2,
      ghosted_book_count: 0,
    },
    worker_queue: [],
    fts: { books_rows: 12, fts_rows: 12, in_sync: true },
    storage: BASE_STORAGE,
    last_errors: [],
  };
  // The analog of "a scan just landed": book count jumps, a ghosted book
  // turns up, and a running scan appears in the worker queue.
  const updated: AdminHealthReport = {
    ...initial,
    index: { ...initial.index, book_count: 47, ghosted_book_count: 3 },
    worker_queue: [{ kind: "scan", queue_depth: 1, recent_completions_ms: [] }],
  };

  const setReport = await mockAdminHealth(page, initial);

  const firstPoll = waitForAdminHealthPoll(page);
  await gotoReady(page, ADMIN_HEALTH);
  await firstPoll;

  const indexCard = page.getByTestId("admin-health-index-card");
  await expect(kvValue(indexCard, "Books indexed")).toHaveText("12");
  await expect(page.getByTestId("admin-health-ghosted-ok")).toBeVisible();
  await expect(page.getByTestId("admin-health-worker-empty")).toBeVisible();

  // Flip the "server-side" report and wait for the *next* poll tick
  // (~POLL_INTERVAL_MS = 5s) to actually fire — not a fixed sleep.
  const secondPoll = waitForAdminHealthPoll(page);
  setReport(updated);
  await secondPoll;

  await expect(kvValue(indexCard, "Books indexed")).toHaveText("47");
  const ghostedWarning = page.getByTestId("admin-health-ghosted-warning");
  await expect(ghostedWarning).toBeVisible();
  await expect(ghostedWarning).toContainText("3 book(s) have no file on disk");

  const workerRow = page.getByTestId("admin-health-worker-row");
  await expect(workerRow).toBeVisible();
  await expect(workerRow).toContainText("Scanning library");

  // No navigation happened — the update landed on the same page.
  await expect(page).toHaveURL(new RegExp(`${ADMIN_HEALTH}$`));
});

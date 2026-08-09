// Worker status indicator (issue #69). The indicator polls
// `/api/rpc/worker_status` at 1 Hz and renders one row per active or
// recently-finished background task above the Save button in the Library Location settings section.
//
// We intercept the polling endpoint instead of seeding a real library so
// the spec doesn't depend on scan duration (which varies wildly with
// fixtures). The interception also lets us drive every render branch —
// idle, active, done, failed — deterministically.

import type { Page, Route } from "@playwright/test";
import { expect, test } from "../fixtures/test";
import { gotoReady } from "../utils/nav";

type ScanTallies = {
  found: number;
  new: number;
  changed: number;
  removed: number;
  moved: number;
  unchanged: number;
};

type WorkerStatus = {
  active: Array<{
    task_id: number;
    kind: "scan" | "generate_thumbs" | "resolve_author_photo";
    state:
      | { state: "running"; processed: number; total?: number | null }
      | { state: "done"; processed: number }
      | { state: "failed"; message: string };
    resource_key?: string | null;
    detail?: {
      phase?: string;
      current_item?: string;
      tallies?: ScanTallies;
    };
    started_at_ms: number;
    last_update_ms: number;
  }>;
  recent_complete: WorkerStatus["active"];
};

const indicator = (page: Page) => page.getByTestId("worker-status");

/**
 * Install a route handler that returns the same canned `WorkerStatus` on
 * every poll. Returns a `setStatus` setter so the test can flip state
 * mid-flow (active → done) without re-installing the route.
 */
async function mockWorkerStatus(
  page: Page,
  initial: WorkerStatus,
): Promise<(next: WorkerStatus) => void> {
  let current: WorkerStatus = initial;
  await page.route("**/api/rpc/worker_status", async (route: Route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(current),
    });
  });
  return (next: WorkerStatus) => {
    current = next;
  };
}

const now = () => Date.now();

test("worker-status indicator stays hidden when the worker is idle", async ({
  page,
}) => {
  await mockWorkerStatus(page, { active: [], recent_complete: [] });
  await gotoReady(page, "/settings?section=library");
  // The indicator renders nothing — no element with the testid is in the DOM.
  await expect(indicator(page)).toHaveCount(0);
});

test("worker-status indicator surfaces an active scan", async ({ page }) => {
  await mockWorkerStatus(page, {
    active: [
      {
        task_id: 1,
        kind: "scan",
        state: { state: "running", processed: 0, total: null },
        resource_key: "/tmp/lib",
        started_at_ms: now(),
        last_update_ms: now(),
      },
    ],
    recent_complete: [],
  });
  await gotoReady(page, "/settings?section=library");

  await expect(indicator(page)).toBeVisible();
  await expect(indicator(page)).toContainText("Scanning library");
});

test("worker-status indicator shows processed/total when total is known", async ({
  page,
}) => {
  await mockWorkerStatus(page, {
    active: [
      {
        task_id: 2,
        kind: "scan",
        state: { state: "running", processed: 3, total: 10 },
        started_at_ms: now(),
        last_update_ms: now(),
      },
    ],
    recent_complete: [],
  });
  await gotoReady(page, "/settings?section=library");

  await expect(indicator(page)).toContainText("Scanning library");
  await expect(indicator(page)).toContainText("(3 / 10)");
});

test("worker-status indicator renders the verbose activity panel for a scan", async ({
  page,
}) => {
  await mockWorkerStatus(page, {
    active: [
      {
        task_id: 7,
        kind: "scan",
        state: { state: "running", processed: 3, total: 10 },
        detail: {
          phase: "Reading file metadata",
          current_item: "books/Bram Stoker/Dracula.epub",
          tallies: {
            found: 340,
            new: 3,
            changed: 1,
            removed: 2,
            moved: 0,
            unchanged: 334,
          },
        },
        started_at_ms: now(),
        last_update_ms: now(),
      },
    ],
    recent_complete: [],
  });
  await gotoReady(page, "/settings?section=library");

  await expect(indicator(page)).toContainText("Scanning library");
  await expect(page.getByTestId("worker-status-phase")).toHaveText(
    "Reading file metadata",
  );
  await expect(page.getByTestId("worker-status-item")).toHaveText(
    "books/Bram Stoker/Dracula.epub",
  );
  await expect(page.getByTestId("worker-status-tallies")).toHaveText(
    "Found 340 — 3 new · 1 changed · 2 removed",
  );
});

test("worker-status done row summarizes the scan's tallies", async ({
  page,
}) => {
  await mockWorkerStatus(page, {
    active: [],
    recent_complete: [
      {
        task_id: 8,
        kind: "scan",
        state: { state: "done", processed: 4 },
        detail: {
          tallies: {
            found: 12,
            new: 3,
            changed: 1,
            removed: 0,
            moved: 0,
            unchanged: 8,
          },
        },
        started_at_ms: now() - 2_000,
        last_update_ms: now(),
      },
    ],
  });
  await gotoReady(page, "/settings?section=library");

  await expect(indicator(page)).toContainText("Library scan");
  await expect(page.getByTestId("worker-status-summary")).toHaveText(
    "3 new · 1 changed",
  );
});

test("worker-status indicator transitions from active to done and lets the user dismiss", async ({
  page,
}) => {
  const setStatus = await mockWorkerStatus(page, {
    active: [
      {
        task_id: 3,
        kind: "scan",
        state: { state: "running", processed: 0, total: null },
        started_at_ms: now(),
        last_update_ms: now(),
      },
    ],
    recent_complete: [],
  });
  await gotoReady(page, "/settings?section=library");
  await expect(indicator(page)).toContainText("Scanning library");

  // Flip the canned response to a terminal Done. Within ~1 polling
  // tick (1s; allow 3s for slow runners) the row reads "Library scan".
  setStatus({
    active: [],
    recent_complete: [
      {
        task_id: 3,
        kind: "scan",
        state: { state: "done", processed: 12 },
        started_at_ms: now() - 1_000,
        last_update_ms: now(),
      },
    ],
  });
  await expect(indicator(page)).toContainText("Library scan", {
    timeout: 3_500,
  });

  // Dismiss the terminal row; the indicator collapses because nothing
  // active is left and the only terminal is suppressed client-side.
  await page.getByRole("button", { name: "Dismiss" }).click();
  await expect(indicator(page)).toHaveCount(0);
});

test("worker-status indicator surfaces a failed scan with the error message", async ({
  page,
}) => {
  await mockWorkerStatus(page, {
    active: [],
    recent_complete: [
      {
        task_id: 4,
        kind: "scan",
        state: { state: "failed", message: "library path does not exist" },
        started_at_ms: now() - 500,
        last_update_ms: now(),
      },
    ],
  });
  await gotoReady(page, "/settings?section=library");

  const failed = page.getByRole("alert");
  await expect(failed).toBeVisible();
  await expect(failed).toContainText("Library scan failed");
  await expect(failed).toContainText("library path does not exist");
});

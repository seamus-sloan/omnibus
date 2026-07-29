import type { Page } from "@playwright/test";
import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { expect, test } from "../fixtures/test";
import { gotoReady } from "../utils/nav";
import { fixturesDir, seedLibrary } from "../utils/seed";

// Persistence: discovery-index sort survives a reload (client-local, via
// `index_prefs`), and scroll position is restored on back-navigation (via
// `scroll_restore`). Both are per-browser-context; each test gets a fresh
// context so localStorage / the in-memory scroll map start clean.
test.beforeAll(async ({ request }) => {
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
});

// ---------------------------------------------------------------------------
// Sort persistence — authors & series
// ---------------------------------------------------------------------------

test("authors sort choice persists across a reload", async ({ page }) => {
  await gotoReady(page, "/authors");

  // Default axis is surname A–Z; switch to "Most books" and persist it.
  await expect(page.getByTestId("authors-sort-name")).toHaveAttribute(
    "aria-pressed",
    "true",
  );
  await page.getByTestId("authors-sort-count").click();
  await expect(page.getByTestId("authors-sort-count")).toHaveAttribute(
    "aria-pressed",
    "true",
  );

  await page.reload();
  await page.waitForLoadState("networkidle");

  // The chosen axis is reloaded from storage after the fetch resolves.
  await expect(page.getByTestId("authors-sort-count")).toHaveAttribute(
    "aria-pressed",
    "true",
  );
  await expect(page.getByTestId("authors-sort-name")).toHaveAttribute(
    "aria-pressed",
    "false",
  );
});

test("series sort choice persists across a reload", async ({ page }) => {
  await gotoReady(page, "/series");

  await expect(page.getByTestId("series-sort-name")).toHaveAttribute(
    "aria-pressed",
    "true",
  );
  await page.getByTestId("series-sort-count").click();
  await expect(page.getByTestId("series-sort-count")).toHaveAttribute(
    "aria-pressed",
    "true",
  );

  await page.reload();
  await page.waitForLoadState("networkidle");

  await expect(page.getByTestId("series-sort-count")).toHaveAttribute(
    "aria-pressed",
    "true",
  );
  await expect(page.getByTestId("series-sort-name")).toHaveAttribute(
    "aria-pressed",
    "false",
  );
});

// ---------------------------------------------------------------------------
// Scroll restoration — back-navigation lands where you left off
// ---------------------------------------------------------------------------

/**
 * Read `window.scrollY` once it has held still across two polls. A restore
 * nudges the offset across animation frames as further pages stream in, so a
 * bare read can catch it mid-converge and hand back a value that has already
 * moved on by the time it's used.
 */
async function settledScrollY(page: Page): Promise<number> {
  let previous = Number.NaN;
  await expect
    .poll(
      async () => {
        const y = await page.evaluate(() => window.scrollY);
        const stable = y === previous;
        previous = y;
        return stable;
      },
      { timeout: 5000 },
    )
    .toBe(true);
  return previous;
}

test("authors index scroll position is restored after visiting a detail", async ({
  page,
}) => {
  // A short viewport guarantees the index overflows and is scrollable even for
  // the small fixture set.
  await page.setViewportSize({ width: 1000, height: 320 });
  await gotoReady(page, "/authors");
  await expect(page.getByTestId("author-card").first()).toBeVisible();

  // Scroll to the bottom, then read the actual (clamped) offset the manager
  // records for this path.
  const savedY = await page.evaluate(() => {
    window.scrollTo(0, document.documentElement.scrollHeight);
    return window.scrollY;
  });
  expect(savedY).toBeGreaterThan(0);

  // Open the *last* author (visible at the bottom, so clicking it doesn't
  // scroll the window and overwrite the saved offset), then navigate back —
  // the index remounts fresh.
  const card = page.getByTestId("author-card").last();
  await card.click();
  await expect(page).toHaveURL(/\/authors\/\d+$/);

  await page.goBack();
  await expect(page.getByTestId("author-card").first()).toBeVisible();

  // Scroll is restored close to where we left off (retry converges after the
  // list re-fetches and paints).
  await expect
    .poll(async () => page.evaluate(() => window.scrollY), { timeout: 5000 })
    .toBeGreaterThan(savedY * 0.6);
});

test("library scroll position is restored after opening a book", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1000, height: 320 });
  await gotoReady(page, "/");

  // Grid tiles navigate on click with no editable cells (unlike admin table
  // rows, where clicking the title cell enters edit mode). Grid is the
  // default view mode, so the tiles are already on screen.
  await expect(page.getByTestId(/^ebook-tile-/).first()).toBeVisible();

  const savedY = await page.evaluate(() => {
    window.scrollTo(0, document.documentElement.scrollHeight);
    return window.scrollY;
  });
  expect(savedY).toBeGreaterThan(0);

  // Open the last tile (visible at the bottom) so the click doesn't scroll.
  await page
    .getByTestId(/^ebook-tile-/)
    .last()
    .click();
  await expect(page).toHaveURL(/\/books\//);

  await page.goBack();
  await expect(page.getByTestId(/^ebook-tile-/).first()).toBeVisible();

  await expect
    .poll(async () => page.evaluate(() => window.scrollY), { timeout: 5000 })
    .toBeGreaterThan(savedY * 0.6);

  // A restore freezes position recording for the length of the navigation —
  // otherwise the browser's own clamped restoration overwrites the saved
  // offset mid-transition. Recording has to come back afterwards, so move
  // somewhere new with a real wheel gesture (which is also what cancels a
  // restore still converging) and leave via the sticky nav, which — unlike
  // clicking an off-screen tile — doesn't move the page on its way out.
  //
  // Size that gesture off where the restore landed, never off `savedY`: the
  // restore only has to get within 40% of it, so a `savedY`-sized delta can
  // overshoot and clamp `scrollY` to 0 — and `restore` bails on a zero
  // target, leaving nothing for the back-navigation below to come back to.
  // The throwaway 1px nudge cancels the converge loop first, so that
  // reference offset can't still be climbing when it's read.
  await page.mouse.move(500, 200);
  await page.mouse.wheel(0, -1);
  const restoredY = await settledScrollY(page);
  expect(restoredY).toBeGreaterThan(0);

  await page.mouse.wheel(0, -Math.round(restoredY / 2));
  const movedY = await settledScrollY(page);
  expect(movedY).toBeGreaterThan(0);
  expect(movedY).toBeLessThan(restoredY);

  await page
    .getByRole("navigation", { name: "Primary" })
    .getByRole("link", { name: "Authors" })
    .click();
  await expect(page).toHaveURL(/\/authors$/);

  await page.goBack();
  await expect(page.getByTestId(/^ebook-tile-/).first()).toBeVisible();

  // The new offset is what comes back. A recorder left frozen by the first
  // restore would still hold the bottom of the grid and land us down there.
  await expect
    .poll(async () => page.evaluate(() => window.scrollY), { timeout: 5000 })
    .toBeGreaterThan(movedY * 0.6);
  expect(await settledScrollY(page)).toBeLessThan(movedY * 1.4);
});

import { expect, test } from "../fixtures/test";
import { AUDIOBOOK_BOOKS, AUDIOBOOK_BOOK_COUNT } from "../fixtures/audiobooks";
import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { expectMutation } from "../utils/api";
import { fetchBookUuidByTitle } from "../utils/ebooks";
import { gotoReady } from "../utils/nav";
import {
  audiobookFixturesDir,
  fixturesDir,
  seedAudiobookLibrary,
  seedLibrary,
} from "../utils/seed";

// Both libraries seeded: the reader-visibility test needs an EPUB to read
// alongside an audiobook playing in the background. No fixture pair shares
// a normalized (title, author), so auto-attach leaves them as separate rows
// and the counts stay additive — same reasoning as `merge.spec.ts`. Seeded
// as two sequential settings writes (not one combined call): a single write
// that reindexes both libraries at once was observed to leave the ebook
// fixture the reader test depends on unresolvable by row testid in CI —
// see the investigation note in the PR. Reverted to the two-step seed.
test.beforeAll(async ({ request }) => {
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
  await seedAudiobookLibrary(
    request,
    audiobookFixturesDir(),
    FIXTURE_BOOKS.length + AUDIOBOOK_BOOK_COUNT,
  );
});

const MP3_BOOK = AUDIOBOOK_BOOKS.find(
  (b) => b.format === "MP3" && b.source === "generated",
)!;
// Distinct author from every audiobook fixture so auto-attach never merges
// this EPUB with the playing audiobook mid-test.
const READER_BOOK = FIXTURE_BOOKS.find((b) => b.slug === "gamma")!;

// The web speed control posts through the `rpc_set_playback_rate` server fn.
const PLAYBACK_RATE_SET_URL = /\/api\/rpc\/audiobooks\/playback-rate\/set(?:\?|$)/;

/**
 * Wait until the App-level playback driver has run `OmnibusAudio.initDirect`
 * for a direct-play book. Mirrors the listen-flow helper — `_mode === "direct"`
 * is the canonical "player is wired and the book metadata is in context" signal.
 */
async function waitForPlayerReady(
  page: import("@playwright/test").Page,
): Promise<void> {
  await expect(page.getByTestId("listen-preparing")).toHaveCount(0);
  await expect
    .poll(
      async () =>
        page.evaluate(() => {
          const audio = (
            window as unknown as { OmnibusAudio?: { _mode?: string | null } }
          ).OmnibusAudio;
          return audio?._mode ?? null;
        }),
      { timeout: 10_000, intervals: [50, 100, 250, 500] },
    )
    .toBe("direct");
}

/**
 * SPA-navigate (client-side) to the library landing page by clicking the
 * full player's top-nav "Library" link. The mini-dock and its backing audio
 * element only persist across true in-app navigation — a full reload remounts
 * `App` and resets the in-memory playback context — so this must click a real
 * Dioxus `Link` (which routes client-side), not a `page.goto` or an injected
 * anchor (which trigger a native page load).
 */
async function spaNavigateToLibrary(
  page: import("@playwright/test").Page,
): Promise<void> {
  await page.getByRole("link", { name: "Library" }).click();
  await expect(page).toHaveURL(/\/$/);
}

// ---------------------------------------------------------------------------
// 1. Dock visibility — absent on the full player, present after navigating away
// ---------------------------------------------------------------------------

test("shows the mini-dock on other pages while an audiobook is loaded", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, MP3_BOOK.title);
  await gotoReady(page, `/listen/${uuid}`);
  await waitForPlayerReady(page);

  // The full player route renders without ScreenLayout, so no dock there.
  await expect(page.getByTestId("mini-dock")).toHaveCount(0);

  // SPA-nav to the library landing page — playback (and the dock) persist.
  await spaNavigateToLibrary(page);

  await expect(page.getByTestId("mini-dock")).toBeVisible();
  await expect(page.getByTestId("mini-dock-title")).toHaveText(MP3_BOOK.title);
});

// ---------------------------------------------------------------------------
// 2. Dock transport reflects global playback state
// ---------------------------------------------------------------------------

test("dock play/pause reflects global playback state and pokes the shared audio", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, MP3_BOOK.title);
  await gotoReady(page, `/listen/${uuid}`);
  await waitForPlayerReady(page);
  await spaNavigateToLibrary(page);

  const toggle = page.getByTestId("mini-dock-toggle");
  await expect(toggle).toBeVisible();
  await expect(toggle).toHaveAttribute("aria-label", "Play");

  // Drive the Rust play callback — the dock reads the shared `playing` signal,
  // so its label must flip without any route change.
  await page.evaluate(() => {
    (
      window as unknown as { __omnibusOnAudioPlay?: (s: number) => void }
    ).__omnibusOnAudioPlay?.(0);
  });
  await expect(toggle).toHaveAttribute("aria-label", "Pause");

  // Clicking the dock toggle drives the same OmnibusAudio surface as the full
  // player — it must exist and the click must not throw.
  const hasToggle = await page.evaluate(
    () =>
      typeof (window as unknown as { OmnibusAudio?: { toggle?: unknown } })
        .OmnibusAudio?.toggle === "function",
  );
  expect(hasToggle).toBe(true);
  await toggle.click();
});

// ---------------------------------------------------------------------------
// 3. Expand returns to the full player and hides the dock
// ---------------------------------------------------------------------------

test("dock expand navigates back to the full player", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, MP3_BOOK.title);
  await gotoReady(page, `/listen/${uuid}`);
  await waitForPlayerReady(page);
  await spaNavigateToLibrary(page);

  await expect(page.getByTestId("mini-dock")).toBeVisible();
  await page.getByTestId("mini-dock-expand-btn").click();

  await expect(page).toHaveURL(new RegExp(`/listen/${uuid}$`));
  await expect(
    page.getByRole("button", { name: "Play", exact: true }),
  ).toBeVisible();
  await expect(page.getByTestId("mini-dock")).toHaveCount(0);
});

// ---------------------------------------------------------------------------
// 3b. Compact bar: single flex row, no volume slider (AC1)
// ---------------------------------------------------------------------------

test("dock is a single-row flex bar with no volume slider", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, MP3_BOOK.title);
  await gotoReady(page, `/listen/${uuid}`);
  await waitForPlayerReady(page);
  await spaNavigateToLibrary(page);

  const dock = page.getByTestId("mini-dock");
  await expect(dock).toBeVisible();

  // The bar is one content-sized flex row now, not the old 4-column grid that
  // wrapped a trailing actions row (the residual bottom whitespace, #1132).
  await expect(dock).toHaveCSS("display", "flex");
  await expect(dock).not.toHaveCSS("flex-wrap", "wrap");

  // The volume slider moved out of the dock in the compact design.
  await expect(page.getByTestId("mini-dock-volume")).toHaveCount(0);
});

// ---------------------------------------------------------------------------
// 3c. Speed chip cycles the rate and stays in sync with the full player
// ---------------------------------------------------------------------------

test("dock speed chip cycles playback rate and reaches the shared audio + full player", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, MP3_BOOK.title);
  await gotoReady(page, `/listen/${uuid}`);
  await waitForPlayerReady(page);
  await spaNavigateToLibrary(page);

  // Speed persists per-book, so don't assume a fixed starting rate — read the
  // chip's current value and assert the transition to the next cycle preset.
  const RATE_CYCLE = [0.8, 1.0, 1.2, 1.5, 1.8, 2.0];
  const speed = page.getByTestId("mini-dock-speed");
  const start = parseFloat((await speed.textContent()) ?? "");
  const next = RATE_CYCLE.find((r) => r > start + 0.001) ?? RATE_CYCLE[0];

  // The chip cycles the rate and persists it via `rpc_set_playback_rate`;
  // assert that POST fired with the next preset before checking UI/audio state.
  await expectMutation(
    page,
    {
      method: "POST",
      url: PLAYBACK_RATE_SET_URL,
      expectedStatus: 200,
      expectedBody: { uuid, update: { playback_rate: next } },
    },
    async () => speed.click(),
  );
  await expect(speed).toHaveText(`${next.toFixed(1)}×`);

  // The chip writes through the shared apply_rate seam, so the change must
  // reach the shared `<audio>` element's playbackRate in real time.
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (document.getElementById("omnibus-audio") as HTMLAudioElement | null)
            ?.playbackRate ?? null,
      ),
    )
    .toBeCloseTo(next, 2);

  // Expanding back to the full player must show the same rate — proof both
  // controls share one signal rather than each tracking its own copy.
  await page.getByTestId("mini-dock-expand-btn").click();
  await expect(page).toHaveURL(new RegExp(`/listen/${uuid}$`));
  await expect(page.getByTestId("listen-rate")).toContainText(next.toFixed(1));
});

// ---------------------------------------------------------------------------
// 3d. Sleep chip expands to the full player (where the sleep timer lives)
// ---------------------------------------------------------------------------

test("dock sleep chip opens the full player", async ({ page, request }) => {
  const uuid = await fetchBookUuidByTitle(request, MP3_BOOK.title);
  await gotoReady(page, `/listen/${uuid}`);
  await waitForPlayerReady(page);
  await spaNavigateToLibrary(page);

  await page.getByTestId("mini-dock-sleep").click();
  await expect(page).toHaveURL(new RegExp(`/listen/${uuid}$`));
});

// ---------------------------------------------------------------------------
// 4. Dismiss stops playback and removes the dock
// ---------------------------------------------------------------------------

test("dock dismiss clears playback and removes the dock", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, MP3_BOOK.title);
  await gotoReady(page, `/listen/${uuid}`);
  await waitForPlayerReady(page);
  await spaNavigateToLibrary(page);

  await expect(page.getByTestId("mini-dock")).toBeVisible();
  await page.getByTestId("mini-dock-dismiss").click();

  await expect(page.getByTestId("mini-dock")).toHaveCount(0);
});

// ---------------------------------------------------------------------------
// 5. Dock shows on the immersive reader — the reader has no transport of
//    its own, so a book playing in the background needs the dock (#988)
// ---------------------------------------------------------------------------

test("shows the mini-dock on the immersive reader while an audiobook is loaded", async ({
  page,
  request,
}) => {
  const audiobookUuid = await fetchBookUuidByTitle(request, MP3_BOOK.title);
  const readerUuid = await fetchBookUuidByTitle(request, READER_BOOK.title);

  await gotoReady(page, `/listen/${audiobookUuid}`);
  await waitForPlayerReady(page);

  // Reach the reader via real Dioxus `Link`/`nav.push` navigation the whole
  // way — a native page load at any hop would remount `App` and drop the
  // in-memory playback context the dock reads from.
  await spaNavigateToLibrary(page);
  // Click the cover cell, not the bare row: a plain `.click()` on the `<tr>`
  // lands at its horizontal center, which is one of the inline-editable
  // cells (authors/series/etc.) — those stop propagation on click, so the
  // row's own navigate-on-click handler never fires. The cover cell has no
  // click handler of its own and safely bubbles to the row.
  await page
    .getByTestId(`ebook-row-${READER_BOOK.slug}`)
    .getByTestId("ebook-cell-cover")
    .click();
  await expect(page).toHaveURL(new RegExp(`/books/${readerUuid}$`));
  await page.getByTestId("start-reading").click();
  await expect(page).toHaveURL(new RegExp(`/read/${readerUuid}$`));

  await expect(page.getByTestId("reader-viewer")).toBeVisible();
  await expect(page.getByTestId("mini-dock")).toBeVisible();
  await expect(page.getByTestId("mini-dock-title")).toHaveText(MP3_BOOK.title);

  // A dock control works identically on the reader as elsewhere (AC3):
  // drive the shared `playing` signal and confirm the toggle reflects it,
  // then confirm a real click reaches the same `OmnibusAudio` surface the
  // other mini-dock tests exercise.
  const toggle = page.getByTestId("mini-dock-toggle");
  await expect(toggle).toHaveAttribute("aria-label", "Play");
  await page.evaluate(() => {
    (
      window as unknown as { __omnibusOnAudioPlay?: (s: number) => void }
    ).__omnibusOnAudioPlay?.(0);
  });
  await expect(toggle).toHaveAttribute("aria-label", "Pause");
  await toggle.click();

  // The reader's own bottom bar and the dock must not visually collide —
  // assert the dock sits above it rather than overlapping (AC2). `.rd-bottom`
  // renders unconditionally in `BookReadPage`, so this is a plain assertion.
  const dockBottom = await page
    .getByTestId("mini-dock")
    .evaluate((el) => el.getBoundingClientRect().bottom);
  const readerBarTop = await page
    .locator(".rd-bottom")
    .evaluate((el) => el.getBoundingClientRect().top);
  expect(dockBottom).toBeLessThanOrEqual(readerBarTop);
});

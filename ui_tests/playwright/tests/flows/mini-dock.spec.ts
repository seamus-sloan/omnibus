import { expect, test } from "../fixtures/test";
import { AUDIOBOOK_BOOKS, AUDIOBOOK_BOOK_COUNT } from "../fixtures/audiobooks";
import { AUTO_ATTACHED_PAIRS } from "../fixtures/dual_format";
import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { expectMutation } from "../utils/api";
import { fetchBookUuidByTitle } from "../utils/ebooks";
import { gotoReady } from "../utils/nav";
import { setRangeValue } from "../utils/sliders";
import {
  audiobookFixturesDir,
  fixturesDir,
  seedAudiobookLibrary,
  seedLibrary,
} from "../utils/seed";

// Both libraries seeded: the reader-visibility test needs an EPUB to read
// alongside an audiobook playing in the background. Exactly one fixture pair
// ("Immersive Voyage") shares a normalized (title, author) and auto-attaches
// into a single dual-format book, so the combined total is the additive sum
// minus AUTO_ATTACHED_PAIRS — same reasoning as `merge.spec.ts`. The books
// this spec drives (gamma + "The Analytical Audiobook") aren't that pair.
// Seeded as two sequential settings writes (not one combined call): a single
// write that reindexes both libraries at once was observed to leave the ebook
// fixture the reader test depends on unresolvable by row testid in CI —
// see the investigation note in the PR. Reverted to the two-step seed.
test.beforeAll(async ({ request }) => {
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
  await seedAudiobookLibrary(
    request,
    audiobookFixturesDir(),
    FIXTURE_BOOKS.length + AUDIOBOOK_BOOK_COUNT - AUTO_ATTACHED_PAIRS,
  );
});

const MP3_BOOK = AUDIOBOOK_BOOKS.find(
  (b) => b.format === "MP3" && b.source === "generated",
)!;
// Multi-part book → one synthetic chapter per part, so the chapter-jump
// buttons have a real boundary to cross (same reasoning as listen.spec.ts).
const MULTIPART_MP3_BOOK = AUDIOBOOK_BOOKS.find(
  (b) => b.format === "MP3" && b.parts > 1,
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

  await expect(page).toHaveURL(new RegExp(`/listen/${uuid}\\??$`));
  await expect(
    page.getByRole("button", { name: "Play", exact: true }),
  ).toBeVisible();
  await expect(page.getByTestId("mini-dock")).toHaveCount(0);
});

// ---------------------------------------------------------------------------
// 3b. Full-width single-row bar with a popover-based volume control
// ---------------------------------------------------------------------------

test("dock is a full-width single-row bar and adjusts volume via its popover", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, MP3_BOOK.title);
  await gotoReady(page, `/listen/${uuid}`);
  await waitForPlayerReady(page);
  await spaNavigateToLibrary(page);

  const dock = page.getByTestId("mini-dock");
  await expect(dock).toBeVisible();

  // One content-sized flex row spanning the full viewport width (the
  // AudioDockBar design — supersedes the old floating-pill geometry).
  await expect(dock).toHaveCSS("display", "flex");
  await expect(dock).not.toHaveCSS("flex-wrap", "wrap");
  const [dockBox, viewport] = await Promise.all([
    dock.boundingBox(),
    page.viewportSize(),
  ]);
  // Edge-to-edge, with a sub-pixel tolerance — boundingBox() is float and can
  // land at N ± 0.5 depending on the renderer's deviceScaleFactor rounding.
  expect(dockBox!.width).toBeCloseTo(viewport!.width, 0);
  expect(dockBox!.x).toBeCloseTo(0, 0);

  // Volume lives behind an icon → upward popover with a vertical slider
  // (supersedes #1132's no-inline-slider AC: the bar row itself still has no
  // slider; the popover does).
  const volPop = page.getByTestId("mini-dock-pop-vol");
  await expect(volPop).not.toHaveClass(/open/);
  await page.getByTestId("mini-dock-volume").click();
  await expect(volPop).toHaveClass(/open/);

  const slider = volPop.getByRole("slider", { name: "Volume" });
  await setRangeValue(slider, 0.3);

  // The popover writes through the shared apply_volume seam, so the change
  // must reach the shared `<audio>` element in real time.
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (document.getElementById("omnibus-audio") as HTMLAudioElement | null)
            ?.volume ?? null,
      ),
    )
    .toBeCloseTo(0.3, 2);

  // Clicking outside (the scrim) closes the popover. (Corner click — the
  // scrim's center can sit under an open panel.)
  await page.getByTestId("mini-dock-scrim").click({ position: { x: 10, y: 10 } });
  await expect(volPop).not.toHaveClass(/open/);
});

// ---------------------------------------------------------------------------
// 3c. Speed chip opens the shared speed panel and stays in sync everywhere
// ---------------------------------------------------------------------------

test("dock speed popover sets playback rate and reaches the shared audio + full player", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, MP3_BOOK.title);
  await gotoReady(page, `/listen/${uuid}`);
  await waitForPlayerReady(page);
  await spaNavigateToLibrary(page);

  // The chip no longer cycles on click — it opens the same preset-grid panel
  // as the full player. Pick a preset different from the persisted rate.
  const speed = page.getByTestId("mini-dock-speed");
  const start = parseFloat((await speed.textContent()) ?? "");
  const next = Math.abs(start - 1.5) > 0.001 ? 1.5 : 1.2;

  const pop = page.getByTestId("mini-dock-pop-speed");
  await expect(pop).not.toHaveClass(/open/);
  await speed.click();
  await expect(pop).toHaveClass(/open/);

  // Selecting a preset persists via `rpc_set_playback_rate`; assert that POST
  // fired with the chosen value before checking UI/audio state.
  await expectMutation(
    page,
    {
      method: "POST",
      url: PLAYBACK_RATE_SET_URL,
      expectedStatus: 200,
      expectedBody: { uuid, update: { playback_rate: next } },
    },
    async () =>
      pop.getByRole("button", { name: `${next.toFixed(1)}×`, exact: true }).click(),
  );
  await expect(speed).toContainText(`${next.toFixed(1)}×`);

  // The panel writes through the shared apply_rate seam, so the change must
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

  // Clicking outside closes the popover; expanding back to the full player
  // must show the same rate — proof both controls share one signal. Click the
  // scrim's top-left corner — its center can sit under the open panel.
  await page.getByTestId("mini-dock-scrim").click({ position: { x: 10, y: 10 } });
  await expect(pop).not.toHaveClass(/open/);
  await page.getByTestId("mini-dock-expand-btn").click();
  await expect(page).toHaveURL(new RegExp(`/listen/${uuid}\\??$`));
  await expect(page.getByTestId("listen-rate")).toContainText(next.toFixed(1));
});

// ---------------------------------------------------------------------------
// 3d. Sleep chip opens the shared sleep panel; the armed timer follows the
//     user into the full player (app-scoped controller)
// ---------------------------------------------------------------------------

test("dock sleep popover arms a countdown that the full player then shows", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, MP3_BOOK.title);
  await gotoReady(page, `/listen/${uuid}`);
  await waitForPlayerReady(page);
  await spaNavigateToLibrary(page);

  const sleep = page.getByTestId("mini-dock-sleep");
  await expect(sleep).toHaveText("Sleep");

  const pop = page.getByTestId("mini-dock-pop-sleep");
  await expect(pop).not.toHaveClass(/open/);
  await sleep.click();
  await expect(pop).toHaveClass(/open/);

  // Arming "30 min" starts the countdown; the chip reflects it live (it
  // begins decrementing immediately, so accept 29:xx or 30:00).
  await pop.getByRole("button", { name: "30 min" }).click();
  await expect(sleep).toHaveText(/^Sleep · (29:\d\d|30:00)$/);

  // The controller is app-scoped: expanding to the full player shows the
  // same running countdown in its sleep status. (Corner click — the scrim's
  // center can sit under the open panel.)
  await page.getByTestId("mini-dock-scrim").click({ position: { x: 10, y: 10 } });
  await page.getByTestId("mini-dock-expand-btn").click();
  await expect(page).toHaveURL(new RegExp(`/listen/${uuid}\\??$`));
  await page.getByRole("button", { name: /^sleep/i }).click();
  await expect(page.getByTestId("sleep-status")).toHaveText(/^(29|30):\d\d$/);
});

// ---------------------------------------------------------------------------
// 3e. Chapter jumps — whole-chapter seeks from the dock transport
// ---------------------------------------------------------------------------

test("dock chapter jumps seek across chapter boundaries", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, MULTIPART_MP3_BOOK.title);
  await gotoReady(page, `/listen/${uuid}`);
  await waitForPlayerReady(page);
  await spaNavigateToLibrary(page);

  // Absolute (cross-part) position, mirroring the shim's `absTime()`: in
  // direct mode the `<audio>` element's currentTime is per-part, so a jump
  // into part 2 would otherwise read as ~0 again.
  const audioTime = () =>
    page.evaluate(() => {
      const oa = (
        window as unknown as {
          OmnibusAudio?: {
            _mode?: string | null;
            _parts?: unknown;
            _cumOffsets?: number[];
            _index?: number;
          };
        }
      ).OmnibusAudio;
      const el = document.getElementById(
        "omnibus-audio",
      ) as HTMLAudioElement | null;
      if (!oa || !el) return null;
      if (oa._mode === "direct" && oa._parts) {
        return (oa._cumOffsets?.[oa._index ?? 0] ?? 0) + (el.currentTime || 0);
      }
      return el.currentTime || 0;
    });
  // Next chapter: from position 0 (chapter 1) the target is chapter 2's
  // start — currentTime must move strictly forward past zero.
  await page.getByTestId("mini-dock-ch-next").click();
  await expect.poll(audioTime).toBeGreaterThan(0);
  const afterNext = (await audioTime())!;

  // Previous chapter: freshly landed at a chapter start (≤ 3 s in), so the
  // target is the previous chapter's start — back to 0.
  await page.getByTestId("mini-dock-ch-prev").click();
  await expect.poll(audioTime).toBeLessThan(afterNext);
});

// ---------------------------------------------------------------------------
// 3f. Popovers are mutually exclusive — opening one closes the others
// ---------------------------------------------------------------------------

test("dock popovers are mutually exclusive", async ({ page, request }) => {
  const uuid = await fetchBookUuidByTitle(request, MP3_BOOK.title);
  await gotoReady(page, `/listen/${uuid}`);
  await waitForPlayerReady(page);
  await spaNavigateToLibrary(page);

  const speedPop = page.getByTestId("mini-dock-pop-speed");
  const sleepPop = page.getByTestId("mini-dock-pop-sleep");

  await page.getByTestId("mini-dock-speed").click();
  await expect(speedPop).toHaveClass(/open/);

  await page.getByTestId("mini-dock-sleep").click();
  await expect(sleepPop).toHaveClass(/open/);
  await expect(speedPop).not.toHaveClass(/open/);

  // Re-clicking the open panel's own chip closes it.
  await page.getByTestId("mini-dock-sleep").click();
  await expect(sleepPop).not.toHaveClass(/open/);
});

// ---------------------------------------------------------------------------
// 3g. Narrow viewports shed the secondary controls, keeping play + actions
// ---------------------------------------------------------------------------

test("dock sheds secondary controls on narrow viewports", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, MP3_BOOK.title);
  await gotoReady(page, `/listen/${uuid}`);
  await waitForPlayerReady(page);
  await spaNavigateToLibrary(page);

  await page.setViewportSize({ width: 700, height: 800 });

  // The `mini-dock-hide` set — chapter jumps, ±30 seeks, volume, speed and
  // sleep chips — disappears below the 900px breakpoint…
  for (const id of [
    "mini-dock-ch-prev",
    "mini-dock-ch-next",
    "mini-dock-skip-back",
    "mini-dock-skip-forward",
    "mini-dock-volume",
    "mini-dock-speed",
    "mini-dock-sleep",
  ]) {
    await expect(page.getByTestId(id)).toBeHidden();
  }

  // …while play, expand, and dismiss stay reachable.
  await expect(page.getByTestId("mini-dock-toggle")).toBeVisible();
  await expect(page.getByTestId("mini-dock-expand-btn")).toBeVisible();
  await expect(page.getByTestId("mini-dock-dismiss")).toBeVisible();
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

  // Immersive geometry: the dock sits flush at the bottom edge and the
  // reader's bottom bar becomes a slim footer pinned directly above it — the
  // two stack rather than collide (#988, restated for the AudioDockBar
  // design). The footer renders unconditionally in `BookReadPage`.
  const dockBox = await page
    .getByTestId("mini-dock")
    .evaluate((el) => el.getBoundingClientRect().toJSON());
  const footerBox = await page
    .getByTestId("reader-footer")
    .evaluate((el) => el.getBoundingClientRect().toJSON());
  const viewport = page.viewportSize()!;
  expect(dockBox.bottom).toBeCloseTo(viewport.height, 0);
  expect(footerBox.bottom).toBeCloseTo(dockBox.top, 0);

  // The reader's own progress ribbon yields to the dock's rail.
  await expect(page.locator(".rd-ribbon")).toBeHidden();
});

import { expect, test } from "../fixtures/test";
import { AUDIOBOOK_BOOKS, AUDIOBOOK_BOOK_COUNT } from "../fixtures/audiobooks";
import { fetchBookUuidByTitle } from "../utils/ebooks";
import { gotoReady } from "../utils/nav";
import { audiobookFixturesDir, seedAudiobookLibrary } from "../utils/seed";
import { setRangeValue } from "../utils/sliders";

test.beforeAll(async ({ request }) => {
  await seedAudiobookLibrary(
    request,
    audiobookFixturesDir(),
    AUDIOBOOK_BOOK_COUNT,
  );
});

const MP3_BOOK = AUDIOBOOK_BOOKS.find(
  (b) => b.format === "MP3" && b.source === "generated",
)!;

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
// 3b. Volume slider stays in sync with the full player (#989)
// ---------------------------------------------------------------------------

test("dock volume slider updates the shared audio element and stays in sync with the full player", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, MP3_BOOK.title);
  await gotoReady(page, `/listen/${uuid}`);
  await waitForPlayerReady(page);
  await spaNavigateToLibrary(page);

  const dockSlider = page
    .getByTestId("mini-dock-volume")
    .getByRole("slider", { name: "Volume" });
  await expect(dockSlider).toBeVisible();
  await expect(dockSlider).toHaveValue("1");

  await setRangeValue(dockSlider, 0.4);

  // Both sliders read/write PlaybackState.volume, so the dock's change must
  // reach the shared `<audio>` element in real time.
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (document.getElementById("omnibus-audio") as HTMLAudioElement | null)
            ?.volume ?? null,
      ),
    )
    .toBeCloseTo(0.4, 2);

  // Expanding back to the full player must show the same volume — proof
  // both controls share one signal rather than each tracking its own copy.
  await page.getByTestId("mini-dock-expand-btn").click();
  await expect(page).toHaveURL(new RegExp(`/listen/${uuid}$`));

  const fullSlider = page.getByRole("slider", { name: "Volume" });
  await expect(fullSlider).toHaveValue("0.4");
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

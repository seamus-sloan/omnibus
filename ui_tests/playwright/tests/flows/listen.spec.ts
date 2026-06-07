import { expect, test } from "../fixtures/test";
import { AUDIOBOOK_BOOKS, AUDIOBOOK_BOOK_COUNT } from "../fixtures/audiobooks";
import { expectMutation } from "../utils/api";
import { fetchBookUuidByTitle } from "../utils/ebooks";
import { gotoReady } from "../utils/nav";
import { audiobookFixturesDir, seedAudiobookLibrary } from "../utils/seed";

// Re-seed in this spec's beforeAll so the running server is indexed against
// the committed audiobook fixtures before any assertion runs — independent
// of whatever other specs in the same worker did before us. We seed
// audiobooks only (not ebooks) so the unified `/api/rpc/ebooks` count is
// exactly the audiobook-group count.
test.beforeAll(async ({ request }) => {
  await seedAudiobookLibrary(request, audiobookFixturesDir(), AUDIOBOOK_BOOK_COUNT);
});

// A fixture with a predictable title that the listen-page heading and
// /api/rpc/ebooks lookup both pin against.
const TARGET = AUDIOBOOK_BOOKS[0]!;

/**
 * Wait until the listen page's manifest fetch has resolved and
 * `OmnibusAudio.initDirect` has been called. The preparing overlay
 * (`listen-preparing`) is rendered while `hls_ready` is false; in direct
 * mode it flips true as soon as the manifest fetch returns, so its
 * absence is the canonical "player is ready to drive" signal.
 */
async function waitForPlayerReady(page: import("@playwright/test").Page): Promise<void> {
  await expect(page.getByTestId("listen-preparing")).toHaveCount(0);
  await expect
    .poll(
      async () =>
        page.evaluate(() => {
          const audio = (window as unknown as { OmnibusAudio?: { _mode?: string | null } })
            .OmnibusAudio;
          return audio?._mode ?? null;
        }),
      {
        message: "OmnibusAudio.initDirect should have run after the manifest fetch",
        timeout: 10_000,
        intervals: [50, 100, 250, 500],
      },
    )
    .toBe("direct");
}

test("renders the listen page layout", async ({ page, request }) => {
  // Deep-link straight to the immersive player by the book's stable uuid,
  // resolved the same way a real click would.
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await gotoReady(page, `/listen/${uuid}`);

  // The listen page is deliberately rendered WITHOUT the app's top-nav
  // chrome — same pattern as /read/:uuid. It owns its own slim top bar
  // with a Back affordance, so the layout test pins that instead. The
  // testid (not getByRole with name "Back") is load-bearing because
  // "Back" is a substring of both "Back 30 seconds" (the skip button)
  // and "Playback speed" (the rate button), so the role+name match is
  // ambiguous in this chrome.
  await expect(page.getByTestId("listen-back")).toBeVisible();
  await expect(page.getByText("Now playing")).toBeVisible();

  // Title and author come from the book metadata via PlayerStage's
  // <h1> and `by {author}` row.
  await expect(page.getByRole("heading", { name: TARGET.title })).toBeVisible();
  await expect(page.getByText(`by ${TARGET.author}`)).toBeVisible();

  // Transport: scrubber + skip-back / play / skip-forward / rate.
  await expect(page.getByRole("slider", { name: "Seek" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Back 30 seconds" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Forward 30 seconds" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Play" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Playback speed" })).toBeVisible();

  // Hidden <audio> element — present in the DOM (display:none) and
  // tagged for direct programmatic access from the action tests.
  await expect(page.getByTestId("listen-audio")).toBeAttached();

  // Direct-mode init flips `hls_ready` true, which removes the preparing
  // overlay; assert that final state so the layout test catches a stuck
  // overlay (the symptom of a broken manifest fetch).
  await waitForPlayerReady(page);
  await expect(page.getByTestId("listen-failed")).toHaveCount(0);
});

test("persists listening progress when the audio element pauses", async ({ page, request }) => {
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await gotoReady(page, `/listen/${uuid}`);
  await waitForPlayerReady(page);

  // The Rust `__omnibusOnAudioPause(secs)` callback (registered in
  // `listen/bootstrap.rs::register_js_callbacks`) fires `post_audio_progress`
  // unconditionally — that's the surface a paused user device exercises on
  // every screen-off. Drive it directly so we don't depend on real audio
  // playback in headless Chromium.
  const PAUSE_AT_SEC = 7.5;
  await expectMutationProgress(
    page,
    { uuid, audioPositionSeconds: PAUSE_AT_SEC, expectedStatus: 200 },
    async () => {
      await page.evaluate((secs) => {
        const cb = (
          window as unknown as { __omnibusOnAudioPause?: (s: number) => void }
        ).__omnibusOnAudioPause;
        if (typeof cb !== "function") {
          throw new Error("__omnibusOnAudioPause not installed");
        }
        cb(secs);
      }, PAUSE_AT_SEC);
    },
  );

  // After a successful pause-driven progress POST, the player chrome stays
  // mounted and no failure overlay appears. The "playing → false" signal
  // does not surface in the SSR markup (the play button label is bound to
  // the `playing` Signal which only flips visibly on a state change), so
  // we assert on overlay state instead — that's the user-visible contract.
  await expect(page.getByTestId("listen-failed")).toHaveCount(0);
  await expect(page.getByRole("heading", { name: TARGET.title })).toBeVisible();
});

test("surfaces a 5xx progress POST without crashing the player", async ({ page, request }) => {
  const uuid = await fetchBookUuidByTitle(request, TARGET.title);
  await gotoReady(page, `/listen/${uuid}`);
  await waitForPlayerReady(page);

  // Intercept the progress POST and force a 500. `post_audio_progress`
  // is fire-and-forget (the local cache is the safety net), so the
  // contract is "the request fired with the expected payload, the
  // server's failure didn't take the page down". The matcher
  // deliberately pins the exact pathname so the sibling
  // `/api/rpc/progress/get` (initial reconciliation) keeps hitting the
  // real server — otherwise the page's mount-time POST would 500 too
  // and we'd be measuring two failures, not one.
  await page.route(
    (url) => url.pathname === "/api/rpc/progress",
    (route) =>
      route.fulfill({
        status: 500,
        contentType: "application/json",
        body: JSON.stringify({ error: "forced-failure" }),
      }),
  );

  const PAUSE_AT_SEC = 12.25;
  await expectMutationProgress(
    page,
    { uuid, audioPositionSeconds: PAUSE_AT_SEC, expectedStatus: 500 },
    async () => {
      await page.evaluate((secs) => {
        const cb = (
          window as unknown as { __omnibusOnAudioPause?: (s: number) => void }
        ).__omnibusOnAudioPause;
        if (typeof cb !== "function") {
          throw new Error("__omnibusOnAudioPause not installed");
        }
        cb(secs);
      }, PAUSE_AT_SEC);
    },
  );

  // Player chrome stays mounted: the failure surfaced only as a logged
  // network error, not as the terminal `listen-failed` overlay (which
  // is reserved for HLS-transcode `failed` and manifest-fetch errors).
  await expect(page.getByTestId("listen-failed")).toHaveCount(0);
  await expect(page.getByRole("heading", { name: TARGET.title })).toBeVisible();
});

// ---------------------------------------------------------------------------
// Local helpers
// ---------------------------------------------------------------------------

interface ProgressMutationOpts {
  uuid: string;
  audioPositionSeconds: number;
  expectedStatus: number;
}

// Match `/api/rpc/progress` exactly, NOT the sibling `/api/rpc/progress/get`
// (initial server reconciliation, fires once on mount) nor
// `/api/rpc/progress/sessions` (session batch — not exercised here). The
// pattern is anchored so a substring containment match in `expectMutation`'s
// URL filter can't accidentally swallow either neighbor.
const PROGRESS_URL = /\/api\/rpc\/progress(?:\?|$)/;

/**
 * Listen-page-flavored thin wrapper around the shared `expectMutation`
 * helper from `utils/api.ts`. Encodes the wire-JSON shape
 * `post_audio_progress` builds:
 *
 *   { "update": { "book_uuid", "format": "audio", "audio_position_seconds" } }
 *
 * so the spec doesn't need to repeat the payload three times. All the
 * actual waiting / pairing / status assertions stay in `expectMutation`
 * per `.claude/rules/04-playwright.md`.
 */
async function expectMutationProgress(
  page: import("@playwright/test").Page,
  opts: ProgressMutationOpts,
  action: () => Promise<void>,
): Promise<void> {
  await expectMutation(
    page,
    {
      method: "POST",
      url: PROGRESS_URL,
      expectedStatus: opts.expectedStatus,
      expectedBody: {
        update: {
          book_uuid: opts.uuid,
          format: "audio",
          audio_position_seconds: opts.audioPositionSeconds,
        },
      },
    },
    action,
  );
}

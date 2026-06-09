import { expect, test } from "../fixtures/test";
import { AUDIOBOOK_BOOKS, AUDIOBOOK_BOOK_COUNT } from "../fixtures/audiobooks";
import { expectMutation } from "../utils/api";
import { fetchBookUuidByTitle } from "../utils/ebooks";
import { gotoReady } from "../utils/nav";
import { audiobookFixturesDir, seedAudiobookLibrary } from "../utils/seed";

test.beforeAll(async ({ request }) => {
  await seedAudiobookLibrary(
    request,
    audiobookFixturesDir(),
    AUDIOBOOK_BOOK_COUNT,
  );
});

const MP3_BOOK = AUDIOBOOK_BOOKS.find((b) => b.format === "MP3" && b.source === "generated")!;
const M4B_BOOK = AUDIOBOOK_BOOKS.find((b) => b.format === "M4B")!;

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

// ---------------------------------------------------------------------------
// 1. Layout — MP3 audiobook
// ---------------------------------------------------------------------------

test("renders the listen page layout for an mp3 audiobook", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, MP3_BOOK.title);
  await gotoReady(page, `/listen/${uuid}`);

  // Hidden <audio> element is in the DOM immediately.
  await expect(page.getByTestId("listen-audio")).toBeAttached();

  // Wait for the preparing overlay to clear — it covers the controls with
  // position:absolute inset:0 until the JS bootstrap calls initDirect.
  await waitForPlayerReady(page);
  await expect(page.getByTestId("listen-failed")).toHaveCount(0);

  // App-wide top-nav is mounted by ReadyPlayer.
  await expect(page.getByRole("navigation", { name: "Primary" })).toBeVisible();

  // "Now playing" kicker above the book title.
  await expect(page.getByText("Now playing")).toBeVisible();

  // Book metadata in the player stage.
  await expect(page.getByRole("heading", { name: MP3_BOOK.title })).toBeVisible();
  await expect(page.getByText(`by ${MP3_BOOK.author}`)).toBeVisible();

  // Transport: scrubber + skip-back / play / skip-forward / rate.
  await expect(page.getByRole("slider", { name: "Seek" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Back 30 seconds" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Forward 30 seconds" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Play", exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Playback speed" })).toBeVisible();
});

// ---------------------------------------------------------------------------
// 2. Layout — M4B audiobook
// ---------------------------------------------------------------------------

test("renders the listen page layout for an m4b audiobook", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, M4B_BOOK.title);
  await gotoReady(page, `/listen/${uuid}`);

  await expect(page.getByRole("button", { name: "Play", exact: true })).toBeVisible();
  await expect(page.getByRole("heading", { name: M4B_BOOK.title })).toBeVisible();
  await expect(page.getByText(`by ${M4B_BOOK.author}`)).toBeVisible();

  await waitForPlayerReady(page);
});

// ---------------------------------------------------------------------------
// 3. Open from book detail
// ---------------------------------------------------------------------------

test("opens the listen page from the book detail Listen action", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, MP3_BOOK.title);
  await gotoReady(page, `/books/${uuid}`);

  await page.getByTestId("action-listen").click();
  await expect(page).toHaveURL(new RegExp(`/listen/${uuid}$`));
  await expect(page.getByRole("button", { name: "Play", exact: true })).toBeVisible();
});

// ---------------------------------------------------------------------------
// 4. SPA-nav between audiobooks resets player signals (#369)
// ---------------------------------------------------------------------------

test("SPA-nav between audiobooks resets player signals (#369)", async ({
  page,
  request,
}) => {
  const uuid1 = await fetchBookUuidByTitle(request, MP3_BOOK.title);
  const uuid2 = await fetchBookUuidByTitle(request, M4B_BOOK.title);

  // Navigate to the first audiobook and wait for the player to render.
  await gotoReady(page, `/listen/${uuid1}`);
  await expect(page.getByRole("heading", { name: MP3_BOOK.title })).toBeVisible();
  await waitForPlayerReady(page);

  // Delay the second audiobook's manifest fetch so the preparing overlay
  // stays visible long enough to assert. Without this, direct-play books
  // flip hls_ready=true in <10 ms and the overlay is never observable.
  // The delay proves hls_ready was actually reset to false by the fix —
  // if it leaked as true from the first book, the overlay would never
  // appear regardless of the delay.
  let releaseManifest!: () => void;
  const manifestGate = new Promise<void>((resolve) => {
    releaseManifest = resolve;
  });
  await page.route(
    (url) => url.pathname.includes("/manifest") && url.pathname.includes(uuid2),
    async (route) => {
      await manifestGate;
      await route.continue();
    },
  );

  // SPA-navigate to the second audiobook via a client-side link click.
  // Dioxus intercepts anchor clicks for same-origin routes, so this
  // triggers a true SPA-nav (no full page reload). The anchor needs
  // visible content + a fixed on-screen position because Playwright's
  // actionability check requires the click target be visible — an empty
  // anchor at the bottom of the document collapses to 0×0 and fails the
  // visibility gate.
  await page.evaluate((url) => {
    const a = document.createElement("a");
    a.href = url;
    a.id = "__test-spa-nav";
    a.textContent = "spa-nav";
    a.style.cssText =
      "position:fixed;top:0;left:0;padding:8px;background:#000;color:#fff;z-index:99999;";
    document.body.appendChild(a);
  }, `/listen/${uuid2}`);
  await page.locator("#__test-spa-nav").click();

  // Wait for the second book's title to appear.
  await expect(page.getByRole("heading", { name: M4B_BOOK.title })).toBeVisible({
    timeout: 10_000,
  });

  // The preparing overlay MUST be visible while the manifest is held —
  // this is the positive proof that hls_ready was reset to false. If the
  // stale hls_ready=true from the first book leaked, this would fail.
  await expect(page.getByTestId("listen-preparing")).toBeVisible();
  await expect(page.getByTestId("listen-failed")).not.toBeVisible();

  // Release the manifest and let the player finish initializing.
  releaseManifest();
  await waitForPlayerReady(page);

  // After init completes: no failure overlay, toggle says "Play".
  await expect(page.getByTestId("listen-failed")).not.toBeVisible();
  await expect(page.getByRole("button", { name: "Play", exact: true })).toBeVisible();
});

// ---------------------------------------------------------------------------
// 5. Progress POST happy path
// ---------------------------------------------------------------------------

test("persists listening progress when the audio element pauses", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, MP3_BOOK.title);
  await gotoReady(page, `/listen/${uuid}`);
  await waitForPlayerReady(page);

  // Drive the Rust __omnibusOnAudioPause callback directly — no real
  // audio playback needed in headless Chromium.
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

  // Player chrome stays mounted, no failure overlay.
  await expect(page.getByTestId("listen-failed")).toHaveCount(0);
  await expect(page.getByRole("heading", { name: MP3_BOOK.title })).toBeVisible();
});

// ---------------------------------------------------------------------------
// 6. Progress POST error path (500)
// ---------------------------------------------------------------------------

test("surfaces a 5xx progress POST without crashing the player", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, MP3_BOOK.title);
  await gotoReady(page, `/listen/${uuid}`);
  await waitForPlayerReady(page);

  // Force 500 on the progress POST only — pin by exact pathname so the
  // sibling `/api/rpc/progress/get` (mount-time reconciliation) keeps
  // hitting the real server.
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

  // Player stays up — fire-and-forget progress uses the local cache as
  // the safety net, not the server response.
  await expect(page.getByTestId("listen-failed")).toHaveCount(0);
  await expect(page.getByRole("heading", { name: MP3_BOOK.title })).toBeVisible();
});

// ---------------------------------------------------------------------------
// 7. Unknown UUID 404
// ---------------------------------------------------------------------------

test("shows not-found message for unknown audiobook uuid", async ({ page }) => {
  await gotoReady(page, "/listen/00000000-0000-0000-0000-000000000000");

  await expect(page.getByText("Audiobook not found.")).toBeVisible();
  await expect(
    page.getByRole("link", { name: "Back to library" }),
  ).toBeVisible();
});

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

interface ProgressMutationOpts {
  uuid: string;
  audioPositionSeconds: number;
  expectedStatus: number;
}

const PROGRESS_URL = /\/api\/rpc\/progress(?:\?|$)/;

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

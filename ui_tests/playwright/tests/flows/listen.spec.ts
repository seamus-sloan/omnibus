import {
  AUDIOBOOK_BOOK_COUNT,
  AUDIOBOOK_BOOKS,
  MERGE_ONLY_TITLES,
  SCRUB_BOOK,
} from "../fixtures/audiobooks";
import { expect, test } from "../fixtures/test";
import { expectMutation } from "../utils/api";
import { fetchBookUuidByTitle } from "../utils/ebooks";
import { gotoReady } from "../utils/nav";
import { audiobookFixturesDir, seedAudiobookLibrary } from "../utils/seed";
import { commitRangeValue, setRangeValue } from "../utils/sliders";

test.beforeAll(async ({ request }) => {
  await seedAudiobookLibrary(
    request,
    audiobookFixturesDir(),
    AUDIOBOOK_BOOK_COUNT,
  );
});

// This spec only ever *reads* its books, so every selector below excludes the
// merge-only fixtures — a merge in flight would change what they assert.
const MP3_BOOK = AUDIOBOOK_BOOKS.find(
  (b) =>
    b.format === "MP3" &&
    b.source === "generated" &&
    !MERGE_ONLY_TITLES.includes(b.title),
)!;
const M4B_BOOK = AUDIOBOOK_BOOKS.find((b) => b.format === "M4B")!;
const MULTIPART_MP3_BOOK = AUDIOBOOK_BOOKS.find(
  (b) => b.format === "MP3" && b.parts > 1,
)!;
const LOCAL_SEED_BOOK = AUDIOBOOK_BOOKS.find(
  (b) => b.format === "MP3" && b.source === "public_domain",
)!;

/**
 * Wait until the listen page's manifest fetch has resolved and
 * `OmnibusAudio.initDirect` has been called. The preparing overlay
 * (`listen-preparing`) is rendered while `hls_ready` is false; in direct
 * mode it flips true as soon as the manifest fetch returns, so its
 * absence is the canonical "player is ready to drive" signal.
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
      {
        message:
          "OmnibusAudio.initDirect should have run after the manifest fetch",
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
  await expect(
    page.getByRole("heading", { name: MP3_BOOK.title }),
  ).toBeVisible();
  await expect(page.getByText(`by ${MP3_BOOK.author}`)).toBeVisible();

  // Transport: chapter map + seek scrubber + skip-back / play / skip-forward / rate / volume.
  await expect(page.getByTestId("chapter-map")).toBeVisible();
  await expect(page.getByRole("slider", { name: "Seek" })).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Back 30 seconds" }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Forward 30 seconds" }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Play", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Playback speed" }),
  ).toBeVisible();
  await expect(page.getByRole("slider", { name: "Volume" })).toBeVisible();
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

  await expect(
    page.getByRole("button", { name: "Play", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: M4B_BOOK.title }),
  ).toBeVisible();
  await expect(page.getByText(`by ${M4B_BOOK.author}`)).toBeVisible();

  await waitForPlayerReady(page);
});

test("persists playback speed per audiobook across reloads", async ({
  page,
  request,
}) => {
  const uuidA = await fetchBookUuidByTitle(request, MP3_BOOK.title);
  const uuidB = await fetchBookUuidByTitle(request, M4B_BOOK.title);
  await gotoReady(page, `/listen/${uuidA}`);
  await waitForPlayerReady(page);

  await page.getByRole("button", { name: "Playback speed" }).click();
  await expectMutation(
    page,
    {
      method: "POST",
      url: PLAYBACK_RATE_SET_URL,
      expectedStatus: 200,
      expectedBody: {
        uuid: uuidA,
        update: { playback_rate: 1.5 },
      },
    },
    async () => page.getByRole("button", { name: "1.5×", exact: true }).click(),
  );
  await expect(page.getByTestId("listen-rate")).toHaveText("1.50×");

  await page.reload();
  await waitForPlayerReady(page);
  await expect(page.getByTestId("listen-rate")).toHaveText("1.50×");

  await gotoReady(page, `/listen/${uuidB}`);
  await waitForPlayerReady(page);
  await expect(page.getByTestId("listen-rate")).toHaveText("1.00×");
});

test("seeds an empty server preference from the account-scoped local speed", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, LOCAL_SEED_BOOK.title);
  const meResponse = await request.get("/api/auth/me");
  expect(meResponse.status()).toBe(200);
  const user = (await meResponse.json()) as { id: number };
  const storageKey = `omn.listening.rate::${user.id}::${uuid}`;
  await page.addInitScript(({ key }) => localStorage.setItem(key, "1.8"), {
    key: storageKey,
  });

  await expectMutation(
    page,
    {
      method: "POST",
      url: PLAYBACK_RATE_SET_URL,
      expectedStatus: 200,
      expectedBody: {
        uuid,
        update: { playback_rate: 1.8 },
      },
    },
    async () => {
      await gotoReady(page, `/listen/${uuid}`);
      await waitForPlayerReady(page);
    },
  );
  await expect(page.getByTestId("listen-rate")).toHaveText("1.80×");

  await page.evaluate((key) => localStorage.removeItem(key), storageKey);
  await page.reload();
  await waitForPlayerReady(page);
  await expect(page.getByTestId("listen-rate")).toHaveText("1.80×");
});

test("surfaces playback speed persistence failures without reverting playback", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, MULTIPART_MP3_BOOK.title);
  await gotoReady(page, `/listen/${uuid}`);
  await waitForPlayerReady(page);
  await page.route(PLAYBACK_RATE_SET_URL, (route) =>
    route.fulfill({
      status: 500,
      contentType: "application/json",
      body: JSON.stringify({ error: "forced-failure" }),
    }),
  );

  await page.getByRole("button", { name: "Playback speed" }).click();
  await expectMutation(
    page,
    {
      method: "POST",
      url: PLAYBACK_RATE_SET_URL,
      expectedStatus: 500,
      expectedBody: {
        uuid,
        update: { playback_rate: 1.8 },
      },
    },
    async () => page.getByRole("button", { name: "1.8×", exact: true }).click(),
  );

  await expect(page.getByTestId("listen-rate")).toHaveText("1.80×");
  await expect(page.getByRole("alert")).toContainText(
    "Could not save playback speed",
  );
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
  // Dioxus serializes the optional `?file_id=` route param as a bare trailing
  // `?` when unset, so allow it (functionally `/listen/:uuid`).
  await expect(page).toHaveURL(new RegExp(`/listen/${uuid}\\??$`));
  await expect(
    page.getByRole("button", { name: "Play", exact: true }),
  ).toBeVisible();
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
  await expect(
    page.getByRole("heading", { name: MP3_BOOK.title }),
  ).toBeVisible();
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
  await expect(page.getByRole("heading", { name: M4B_BOOK.title })).toBeVisible(
    {
      timeout: 10_000,
    },
  );

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
  await expect(
    page.getByRole("button", { name: "Play", exact: true }),
  ).toBeVisible();
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
  await expect(
    page.getByRole("heading", { name: MP3_BOOK.title }),
  ).toBeVisible();
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
  await expect(
    page.getByRole("heading", { name: MP3_BOOK.title }),
  ).toBeVisible();
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
// 8. Sleep panel
// ---------------------------------------------------------------------------

test("opens sleep panel and shows preset duration rail", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, MP3_BOOK.title);
  await gotoReady(page, `/listen/${uuid}`);
  await waitForPlayerReady(page);

  // Open the sleep panel via the toolbar button.
  await page.getByRole("button", { name: /^sleep/i }).click();

  // Panel container is in the DOM.
  const panel = page.getByTestId("sleep-panel");
  await expect(panel).toBeVisible();

  // Preset rail: Off through 4 hours. Scope to the panel — the toolbar Sleep
  // button reads "Sleep · off" when no timer is armed, which would otherwise
  // also match a bare { name: "Off" } and trip Playwright's strict mode.
  await expect(panel.getByRole("button", { name: "Off" })).toBeVisible();
  await expect(panel.getByRole("button", { name: "15 min" })).toBeVisible();
  await expect(panel.getByRole("button", { name: "30 min" })).toBeVisible();
  await expect(panel.getByRole("button", { name: "45 min" })).toBeVisible();
  await expect(panel.getByRole("button", { name: "1 hour" })).toBeVisible();
  await expect(panel.getByRole("button", { name: "2 hours" })).toBeVisible();
  await expect(panel.getByRole("button", { name: "3 hours" })).toBeVisible();
  await expect(panel.getByRole("button", { name: "4 hours" })).toBeVisible();
  await expect(
    panel.getByRole("button", { name: "End of chapter" }),
  ).toBeVisible();
});

// ---------------------------------------------------------------------------
// 9. Bookmarks drawer
// ---------------------------------------------------------------------------

test("opens bookmarks drawer and shows empty state", async ({
  page,
  request,
}) => {
  // Use the multipart generated MP3 — it reaches direct-play mode reliably in
  // headless (unlike the large public-domain M4B), and no other spec creates
  // bookmarks against it, so the empty state holds on the shared dev DB. (The
  // create flow below bookmarks MP3_BOOK, so reusing that here would be flaky.)
  const uuid = await fetchBookUuidByTitle(request, MULTIPART_MP3_BOOK.title);
  await gotoReady(page, `/listen/${uuid}`);
  await waitForPlayerReady(page);

  // Open the bookmarks drawer via the toolbar button.
  await page.getByRole("button", { name: "Bookmark" }).click();

  // Drawer container is in the DOM.
  await expect(page.getByTestId("bookmarks-drawer")).toBeVisible();

  // Empty state copy — no bookmarks have been saved yet.
  await expect(page.getByText("No bookmarks yet")).toBeVisible();
  await expect(
    page.getByText("Tap the Bookmark button while listening to save"),
  ).toBeVisible();
});

// ---------------------------------------------------------------------------
// 9b. Bookmark create → toast → row (PR4)
// ---------------------------------------------------------------------------

test("saves a bookmark from the drawer and shows a confirmation toast", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, MP3_BOOK.title);
  await gotoReady(page, `/listen/${uuid}`);
  await waitForPlayerReady(page);

  // Open the drawer, then add a bookmark at the current position. The create
  // RPC must fire with the book uuid before the row + toast appear.
  await page.getByRole("button", { name: "Bookmark" }).click();
  await expect(page.getByTestId("bookmarks-drawer")).toBeVisible();

  await expectMutation(
    page,
    {
      method: "POST",
      url: /\/api\/rpc\/bookmarks\/create(?:\?|$)/,
      expectedStatus: 200,
    },
    async () => page.getByTestId("bookmark-add").click(),
  );

  // The freshly-created bookmark row and the confirmation toast appear.
  await expect(page.getByTestId("bookmark-row").first()).toBeVisible();
  await expect(page.getByTestId("bookmark-toast")).toBeVisible();
  await expect(page.getByText("Bookmark saved")).toBeVisible();
});

// ---------------------------------------------------------------------------
// 8b. Sleep timer countdown (PR5)
// ---------------------------------------------------------------------------

test("arms a sleep preset and shows a live countdown", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, MP3_BOOK.title);
  await gotoReady(page, `/listen/${uuid}`);
  await waitForPlayerReady(page);

  await page.getByRole("button", { name: /^sleep/i }).click();
  await expect(page.getByTestId("sleep-panel")).toBeVisible();

  // Selecting "15 min" arms the timer; the status shows a sub-15-minute
  // countdown (it begins decrementing immediately).
  await page.getByRole("button", { name: "15 min" }).click();
  await expect(page.getByTestId("sleep-status")).toHaveText(/^1[45]:\d\d$/);

  // The toolbar Sleep button reflects the live countdown.
  await expect(
    page.getByRole("button", { name: /^Sleep · 1[45]:/ }),
  ).toBeVisible();
});

// ---------------------------------------------------------------------------
// 9c. Volume slider (#989)
// ---------------------------------------------------------------------------

test("adjusts volume via the full player's slider", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, MP3_BOOK.title);
  await gotoReady(page, `/listen/${uuid}`);
  await waitForPlayerReady(page);

  const slider = page.getByRole("slider", { name: "Volume" });
  await expect(slider).toBeVisible();
  await expect(slider).toHaveValue("1");

  await setRangeValue(slider, 0.3);

  // The slider drives the shared `<audio>` element's volume in real time.
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (document.getElementById("omnibus-audio") as HTMLAudioElement | null)
            ?.volume ?? null,
      ),
    )
    .toBeCloseTo(0.3, 2);
  await expect(slider).toHaveValue("0.3");
});

// ---------------------------------------------------------------------------
// 9d. Seek scrubber (YouTube-style drag-to-seek)
// ---------------------------------------------------------------------------

// The scrubber previews on drag (`input`) but only seeks + persists on release
// (`change`). Dragging must not move the audio; releasing must seek to the
// exact target and POST the new position.
test("scrubs to an arbitrary position and seeks only on release", async ({
  page,
  request,
}) => {
  // SCRUB_BOOK is reserved for this spec: seeking persists a position, so no
  // other spec may read it (see fixtures/audiobooks.ts).
  const uuid = await fetchBookUuidByTitle(request, SCRUB_BOOK.title);
  await gotoReady(page, `/listen/${uuid}`);
  await waitForPlayerReady(page);

  const seek = page.getByRole("slider", { name: "Seek" });
  await expect(seek).toBeVisible();

  const audioPos = () =>
    page.evaluate(
      () =>
        (document.getElementById("omnibus-audio") as HTMLAudioElement | null)
          ?.currentTime ?? -1,
    );

  // Normalize the start position. Scrubbing persists a per-(user, book)
  // position, so a previous run — or a CI retry — can resume this book
  // mid-way, which would defeat the "preview doesn't move the audio" baseline
  // below. Poke the shim to seek back to 0 and wait for it to settle.
  await page.evaluate(() => {
    (
      window as unknown as { OmnibusAudio?: { seek(s: number): void } }
    ).OmnibusAudio?.seek(0);
  });
  await expect.poll(audioPos).toBeLessThan(1);

  // Target ~60% in — a clear mid-book position. SCRUB_BOOK is single-part, so
  // the audio element's currentTime is already the absolute position.
  const max = Number(await seek.getAttribute("max"));
  const target = Math.round(max * 0.6);

  // Drag-preview (input only): the bar previews but the audio must NOT seek.
  await setRangeValue(seek, target);
  // Let any (incorrect) async seek settle before asserting — a bare
  // synchronous read could let a next-tick seek slip through as a false pass.
  await page.evaluate(
    () =>
      new Promise<void>((resolve) =>
        requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
      ),
  );
  expect(await audioPos(), "preview must not move the audio").toBeLessThan(1);

  // Release (input + change): now it seeks and persists the new position. The
  // POST body isn't asserted exactly — MP3 seeks snap to frame boundaries, so
  // the persisted position is target ± a frame — but the request must fire.
  const { request: progressReq } = await expectMutation(
    page,
    {
      method: "POST",
      url: /\/api\/rpc\/progress(?:\?|$)/,
      expectedStatus: 200,
    },
    async () => commitRangeValue(seek, target),
  );
  const body = progressReq.postDataJSON() as {
    update: { format: string; audio_position_seconds: number };
  };
  expect(body.update.format).toBe("audio");
  expect(Math.abs(body.update.audio_position_seconds - target)).toBeLessThan(2);

  // The audio element actually moved to (near) the target.
  await expect
    .poll(async () => Math.abs((await audioPos()) - target))
    .toBeLessThan(2);
});

// ---------------------------------------------------------------------------
// 10. Chapters drawer
// ---------------------------------------------------------------------------

test("opens chapters drawer and shows played/current/upcoming row states", async ({
  page,
  request,
}) => {
  // Use the multi-part MP3 book so at least two synthetic chapters are
  // present (one per part): the first is "current" at elapsed=0, the
  // second is "upcoming". The synthetic fallback always produces
  // file_chapters.len() >= 1, so even books without embedded markers work.
  const uuid = await fetchBookUuidByTitle(request, MULTIPART_MP3_BOOK.title);
  await gotoReady(page, `/listen/${uuid}`);
  await waitForPlayerReady(page);

  // Open the chapters drawer via the toolbar button.
  await page.getByRole("button", { name: /^chapters/i }).click();

  // Drawer container is in the DOM.
  await expect(page.getByTestId("chapters-drawer")).toBeVisible();

  // At elapsed=0 the first chapter is current and the rest are upcoming.
  // getByTestId returns all matching elements; we need at least one current
  // and at least one upcoming row.
  await expect(page.getByTestId("chapter-row-current").first()).toBeVisible();
  await expect(page.getByTestId("chapter-row-upcoming").first()).toBeVisible();
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
const PLAYBACK_RATE_SET_URL =
  /\/api\/rpc\/audiobooks\/playback-rate\/set(?:\?|$)/;

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

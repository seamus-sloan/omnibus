import { expect, test } from "../fixtures/test";
import {
  EXPECTED_AUDIOBOOK_COUNT,
  FIXTURE_AUDIOBOOKS,
} from "../fixtures/audiobooks";
import { fetchBookUuidByTitle } from "../utils/ebooks";
import { gotoReady } from "../utils/nav";
import { audiobookFixturesDir, seedAudiobookLibrary } from "../utils/seed";

test.beforeAll(async ({ request }) => {
  await seedAudiobookLibrary(
    request,
    audiobookFixturesDir(),
    EXPECTED_AUDIOBOOK_COUNT,
  );
});

const MP3_BOOK = FIXTURE_AUDIOBOOKS.find((b) => b.format === "MP3")!;
const M4B_BOOK = FIXTURE_AUDIOBOOKS.find((b) => b.format === "M4B")!;

test("renders the listen page layout for an mp3 audiobook", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, MP3_BOOK.title);
  await gotoReady(page, `/listen/${uuid}`);

  // Top bar with back button and "Now playing" label.
  await expect(page.getByTestId("listen-back")).toBeVisible();
  await expect(page.getByText("Now playing")).toBeVisible();

  // Hidden audio element is present in the DOM.
  await expect(page.getByTestId("listen-audio")).toBeAttached();

  // Transport controls: skip back, play/pause toggle, skip forward, rate.
  await expect(page.getByTestId("listen-skip-back")).toBeVisible();
  await expect(page.getByTestId("listen-toggle")).toBeVisible();
  await expect(page.getByTestId("listen-skip-forward")).toBeVisible();
  await expect(page.getByTestId("listen-rate")).toBeVisible();

  // Scrub bar.
  await expect(page.getByTestId("listen-scrub")).toBeVisible();

  // Book metadata rendered in the stage. The title appears both in the cover
  // fallback plate (`<div class="ct">`) and the player heading, so scope to
  // the heading to avoid a strict-mode collision.
  await expect(
    page.getByRole("heading", { name: MP3_BOOK.title }),
  ).toBeVisible();
  await expect(page.getByText(`by ${MP3_BOOK.artist}`)).toBeVisible();
});

test("renders the listen page layout for an m4b audiobook", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, M4B_BOOK.title);
  await gotoReady(page, `/listen/${uuid}`);

  await expect(page.getByTestId("listen-toggle")).toBeVisible();
  await expect(
    page.getByRole("heading", { name: M4B_BOOK.title }),
  ).toBeVisible();
  await expect(page.getByText(`by ${M4B_BOOK.artist}`)).toBeVisible();
});

test("opens the listen page from the book detail Listen action", async ({
  page,
  request,
}) => {
  const uuid = await fetchBookUuidByTitle(request, MP3_BOOK.title);
  await gotoReady(page, `/books/${uuid}`);

  await page.getByTestId("action-listen").click();
  await expect(page).toHaveURL(new RegExp(`/listen/${uuid}$`));
  await expect(page.getByTestId("listen-toggle")).toBeVisible();
});

test("SPA-nav between audiobooks resets player signals (#369)", async ({
  page,
  request,
}) => {
  const uuid1 = await fetchBookUuidByTitle(request, MP3_BOOK.title);
  const uuid2 = await fetchBookUuidByTitle(request, M4B_BOOK.title);

  // Navigate to the first audiobook and wait for the player to render.
  await gotoReady(page, `/listen/${uuid1}`);
  await expect(page.getByTestId("listen-toggle")).toBeVisible();
  await expect(
    page.getByRole("heading", { name: MP3_BOOK.title }),
  ).toBeVisible();

  // SPA-navigate to the second audiobook via a client-side link click.
  // Dioxus intercepts anchor clicks for same-origin routes, so this
  // triggers a true SPA-nav (no full page reload). The back button is
  // always present as an anchor, so we inject a temporary link.
  await page.evaluate((url) => {
    const a = document.createElement("a");
    a.href = url;
    a.id = "__test-spa-nav";
    document.body.appendChild(a);
  }, `/listen/${uuid2}`);
  await page.locator("#__test-spa-nav").click();

  // Wait for the second book's title to appear, confirming the page updated.
  await expect(
    page.getByRole("heading", { name: M4B_BOOK.title }),
  ).toBeVisible({ timeout: 10_000 });

  // The fix from #369: stale signals from the first book must not leak.
  // The preparing overlay should not be hidden by a stale hls_ready=true,
  // and a stale playback_failed=true should not flash a failure overlay.
  // We can't assert the overlay appeared and disappeared (it's transient
  // for direct-play books), but we CAN assert the failure overlay is NOT
  // shown — if stale playback_failed leaked, it would be visible.
  await expect(page.getByTestId("listen-failed")).not.toBeVisible();

  // The toggle button should still say "Play" (not stuck in a stale
  // playing=true state from the previous book).
  await expect(page.getByTestId("listen-toggle")).toHaveText("Play");
});

test("shows 404-style message for unknown audiobook uuid", async ({
  page,
}) => {
  await gotoReady(page, "/listen/00000000-0000-0000-0000-000000000000");

  await expect(page.getByText("Audiobook not found.")).toBeVisible();
  await expect(page.getByRole("link", { name: "Back to library" })).toBeVisible();
});

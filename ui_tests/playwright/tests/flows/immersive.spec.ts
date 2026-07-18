// F-1130 (AC4): Immersive Read — open the EPUB reader with the audiobook
// player docked, for a book that has BOTH formats.
//
// The precondition is a single book carrying an ebook AND an audiobook. The
// "Immersive Voyage" fixture is the one intentional (title, author) collision
// across the two libraries, so the indexer auto-attaches its EPUB and MP3 into
// one dual-format book (see `fixtures/dual_format.ts`). Both libraries are
// therefore seeded here, as two sequential settings writes (mirroring
// `mini-dock.spec.ts`).
import { AUDIOBOOK_BOOK_COUNT } from "../fixtures/audiobooks";
import { AUTO_ATTACHED_PAIRS, DUAL_FORMAT_BOOK } from "../fixtures/dual_format";
import { FIXTURE_BOOKS } from "../fixtures/epubs";
import { expect, test } from "../fixtures/test";
import { expectNavVisible, gotoReady } from "../utils/nav";
import {
  audiobookFixturesDir,
  fixturesDir,
  seedAudiobookLibrary,
  seedLibrary,
} from "../utils/seed";
import type { APIRequestContext } from "@playwright/test";

test.beforeAll(async ({ request }) => {
  await seedLibrary(request, fixturesDir(), FIXTURE_BOOKS.length);
  await seedAudiobookLibrary(
    request,
    audiobookFixturesDir(),
    FIXTURE_BOOKS.length + AUDIOBOOK_BOOK_COUNT - AUTO_ATTACHED_PAIRS,
  );
});

/**
 * Resolve the dual-format book's uuid once the indexer has attached BOTH
 * formats to it. Seeding gates on a combined *count*, which a single-format
 * scan can satisfy before the second library's scan attaches the other
 * format — so poll `/api/rpc/ebooks` until the book reports an ebook format
 * *and* an audio format, then return its uuid. Without this the Immersive
 * Read CTA (gated on `has_ebook && has_audio`) may not have rendered yet.
 */
async function fetchDualFormatUuid(request: APIRequestContext): Promise<string> {
  const isEbook = (f: string) => /^(epub|pdf)$/i.test(f);
  const isAudio = (f: string) => /^(mp3|m4b)$/i.test(f);

  let match: { unique_identifier: string | null; formats: string[] } | undefined;
  await expect
    .poll(
      async () => {
        const resp = await request.get("/api/rpc/ebooks");
        if (resp.status() !== 200) return false;
        const body = (await resp.json()) as {
          books: {
            title: string | null;
            unique_identifier: string | null;
            formats: string[];
          }[];
        };
        match = body.books.find((b) => b.title === DUAL_FORMAT_BOOK.title);
        return (
          match !== undefined &&
          match.formats.some(isEbook) &&
          match.formats.some(isAudio)
        );
      },
      {
        message: `book ${JSON.stringify(DUAL_FORMAT_BOOK.title)} never surfaced with both an ebook and an audio format`,
        timeout: 20_000,
        intervals: [100, 200, 500, 1_000, 2_000],
      },
    )
    .toBe(true);

  if (!match?.unique_identifier) {
    throw new Error(`dual-format book ${JSON.stringify(DUAL_FORMAT_BOOK.title)} is missing its uuid`);
  }
  return match.unique_identifier;
}

// ---------------------------------------------------------------------------
// Layout — the dual-format CTA row carries the Immersive Read button
// ---------------------------------------------------------------------------

test("renders the Immersive Read CTA on a dual-format book detail", async ({
  page,
  request,
}) => {
  const uuid = await fetchDualFormatUuid(request);
  await gotoReady(page, `/books/${uuid}`);

  await expectNavVisible(page);

  // Both single-format CTAs plus the both-formats-only Immersive Read button.
  await expect(page.getByTestId("start-reading")).toBeVisible();
  await expect(page.getByTestId("listen-secondary")).toBeVisible();

  const immersive = page.getByTestId("immersive-read");
  await expect(immersive).toBeVisible();
  await expect(immersive).toHaveText(/Immersive Read/);
});

// ---------------------------------------------------------------------------
// Action — Immersive Read opens the reader with the audiobook docked
// ---------------------------------------------------------------------------

test("Immersive Read opens the reader with the audiobook player docked", async ({
  page,
  request,
}) => {
  const uuid = await fetchDualFormatUuid(request);
  await gotoReady(page, `/books/${uuid}`);

  // The button retargets the app-wide playback state at this book and
  // client-side navigates to the reader — a real rsx onclick, so the page
  // must be hydrated (gotoReady waited on networkidle). No mutation fires;
  // the manifest load is a GET, so assert the resulting UI state directly.
  await page.getByTestId("immersive-read").click();
  await expect(page).toHaveURL(new RegExp(`/read/${uuid}$`));

  // The reader renders the EPUB and the mini-dock surfaces the SAME book's
  // audiobook — the reader has no transport of its own, so the docked player
  // is how "read + listen together" is exercised.
  await expect(page.getByTestId("reader-viewer")).toBeVisible();
  const dock = page.getByTestId("mini-dock");
  await expect(dock).toBeVisible();
  await expect(page.getByTestId("mini-dock-title")).toHaveText(DUAL_FORMAT_BOOK.title);
});

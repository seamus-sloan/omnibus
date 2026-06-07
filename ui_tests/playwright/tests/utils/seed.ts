import { expect, type APIRequestContext } from "@playwright/test";
import { resolve } from "node:path";

/**
 * Absolute path to the committed EPUB fixtures (`<repo>/test_data/epubs`).
 * The scanner recurses, so this picks up both the deterministic synthetic
 * EPUBs under `generated/` and the public-domain EPUBs under
 * `public_domain/`. Resolved from this file's `__dirname` so the spec
 * doesn't care which working directory Playwright was launched from.
 */
export function fixturesDir(): string {
  // tests/utils/ -> tests/ -> playwright/ -> ui_tests/ -> <repo>
  return resolve(__dirname, "..", "..", "..", "..", "test_data", "epubs");
}

/**
 * Absolute path to the committed audiobook fixtures
 * (`<repo>/test_data/audiobooks`). The audiobook indexer walks this tree
 * the same way the ebook scanner walks `epubs/`: a single-file `.mp3` (or
 * `.m4b`/`.m4a`) becomes a one-part group, and a folder of per-chapter
 * `.mp3`s becomes one multi-part group keyed by that folder. See
 * `db::audiobook::group_into_books`.
 */
export function audiobookFixturesDir(): string {
  return resolve(__dirname, "..", "..", "..", "..", "test_data", "audiobooks");
}

/**
 * Seed the running server: POST the fixtures path(s) to `/api/rpc/settings`,
 * then poll `GET /api/rpc/ebooks` until the indexer has surfaced
 * `expectedCount` books. Indexing runs in a `tokio::spawn` after the settings
 * write returns, so we cannot rely on the POST response alone.
 *
 * `expectedCount` is the *total* number of books `/api/rpc/ebooks` should
 * return — both ebooks and audiobooks ride on the unified landing-page
 * query (`db::library_from_db_combined`). When `audiobookLibraryPath` is
 * `null` (the historical shape) the count is just the ebook total.
 */
export async function seedLibrary(
  request: APIRequestContext,
  ebookLibraryPath: string,
  expectedCount: number,
  audiobookLibraryPath: string | null = null,
): Promise<void> {
  const settingsResp = await request.post("/api/rpc/settings", {
    data: {
      settings: {
        ebook_library_path: ebookLibraryPath,
        audiobook_library_path: audiobookLibraryPath,
      },
    },
  });
  expect(settingsResp.status(), "POST /api/rpc/settings failed").toBe(200);

  // Poll until the indexer's reindex task has populated the DB. The 45s
  // budget covers cold starts on the GitHub-hosted Linux runner, where
  // indexing the full public-domain set (including an 84 MB Count of Monte
  // Cristo) measured 27–36s in practice. Healthy local runs settle in <1s.
  await expect
    .poll(
      async () => {
        const resp = await request.get("/api/rpc/ebooks");
        if (resp.status() !== 200) return -1;
        const body = (await resp.json()) as { books?: unknown[] };
        return Array.isArray(body.books) ? body.books.length : -1;
      },
      {
        message: `expected ${expectedCount} books from /api/rpc/ebooks after seeding`,
        timeout: 45_000,
        intervals: [100, 200, 500, 1_000, 2_000],
      },
    )
    .toBe(expectedCount);
}

/**
 * Seed only the audiobook library — POST the path to `/api/rpc/settings`
 * with `ebook_library_path: null`, then poll `/api/rpc/ebooks` for
 * `expectedCount` audiobook *groups*. Use from the listen spec, which
 * doesn't need any EPUB fixtures indexed alongside.
 *
 * The unified landing-page query (`db::library_from_db_combined`)
 * counts audiobook groups (one per folder of mp3s, one per single
 * `.m4b`/`.m4a`/`.mp3`), not individual parts — see
 * `db::audiobook::group_into_books`.
 */
export async function seedAudiobookLibrary(
  request: APIRequestContext,
  audiobookLibraryPath: string,
  expectedCount: number,
): Promise<void> {
  const settingsResp = await request.post("/api/rpc/settings", {
    data: {
      settings: {
        ebook_library_path: null,
        audiobook_library_path: audiobookLibraryPath,
      },
    },
  });
  expect(settingsResp.status(), "POST /api/rpc/settings failed").toBe(200);

  await expect
    .poll(
      async () => {
        const resp = await request.get("/api/rpc/ebooks");
        if (resp.status() !== 200) return -1;
        const body = (await resp.json()) as { books?: unknown[] };
        return Array.isArray(body.books) ? body.books.length : -1;
      },
      {
        message: `expected ${expectedCount} audiobook groups from /api/rpc/ebooks after seeding`,
        timeout: 45_000,
        intervals: [100, 200, 500, 1_000, 2_000],
      },
    )
    .toBe(expectedCount);
}

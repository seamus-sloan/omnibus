import { expect, type APIRequestContext } from "@playwright/test";
import { resolve } from "node:path";

/**
 * Absolute path to the committed EPUB fixtures
 * (`<repo>/test-data/epubs/generated`). Resolved from this file's `__dirname`
 * so the spec doesn't care which working directory Playwright was launched
 * from.
 */
export function fixturesDir(): string {
  // tests/utils/ -> tests/ -> playwright/ -> ui_tests/ -> <repo>
  return resolve(__dirname, "..", "..", "..", "..", "test-data", "epubs", "generated");
}

/**
 * Seed the running server: POST the fixtures path to `/api/rpc/settings`,
 * then poll `GET /api/rpc/ebooks` until the indexer has surfaced
 * `expectedCount` books. Indexing runs in a `tokio::spawn` after the settings
 * write returns, so we cannot rely on the POST response alone.
 */
export async function seedLibrary(
  request: APIRequestContext,
  ebookLibraryPath: string,
  expectedCount: number,
): Promise<void> {
  const settingsResp = await request.post("/api/rpc/settings", {
    data: {
      settings: {
        ebook_library_path: ebookLibraryPath,
        audiobook_library_path: null,
      },
    },
  });
  expect(settingsResp.status(), "POST /api/rpc/settings failed").toBe(200);

  // Poll until the indexer's reindex task has populated the DB. ~15s budget
  // covers cold starts; healthy runs settle in <1s.
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
        timeout: 15_000,
        intervals: [100, 200, 500, 1_000],
      },
    )
    .toBe(expectedCount);
}

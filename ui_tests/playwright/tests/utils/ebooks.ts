import type { APIRequestContext, Locator, Page } from "@playwright/test";
import type { ExpectedBook } from "../fixtures/epubs";
import { expect } from "../fixtures/test";

/**
 * Look up the backend `uuid` for a fixture book by exact title. Hits the
 * same RPC the landing page reads, so the uuid matches what
 * `/books/:uuid` would receive on a real click. Throws if the seeded
 * library does not contain a book with that title.
 *
 * The route is uuid-keyed (not id-keyed) so bookmarked URLs survive
 * reindexes — `books.id` is `AUTOINCREMENT` and renumbers on every
 * `replace_books` run, while `books.uuid` is deterministic UUIDv5 of
 * `(library_path, filename)` and stays stable across reindexes and
 * re-installs. See `db::queries::stable_uuid`.
 */
export async function fetchBookUuidByTitle(
  request: APIRequestContext,
  title: string,
): Promise<string> {
  // Poll rather than read once. `seedLibrary`/`seedAudiobookLibrary` gate on a
  // *total* book count from `/api/rpc/ebooks`, which a parallel spec's ebooks
  // can satisfy before this spec's own reindex has surfaced `title` — so a
  // single GET here races the indexer and throws "no seeded book". Retrying
  // until the specific title appears absorbs that lag; a genuinely-absent book
  // still fails after the timeout with the same diagnostic.
  let lastCount = -1;
  let match:
    | { unique_identifier: string | null; title: string | null }
    | undefined;
  await expect
    .poll(
      async () => {
        const resp = await request.get("/api/rpc/ebooks");
        if (resp.status() !== 200) return false;
        const body = (await resp.json()) as {
          books: { unique_identifier: string | null; title: string | null }[];
        };
        lastCount = body.books.length;
        match = body.books.find((b) => b.title === title);
        return match !== undefined;
      },
      {
        message: `no seeded book with title ${JSON.stringify(title)} surfaced by /api/rpc/ebooks`,
        timeout: 15_000,
        intervals: [100, 200, 500, 1_000],
      },
    )
    .toBe(true);

  if (!match) {
    throw new Error(
      `no seeded book with title ${JSON.stringify(title)} (got ${lastCount} books)`,
    );
  }
  if (!match.unique_identifier) {
    throw new Error(`book ${JSON.stringify(title)} is missing its uuid`);
  }
  return match.unique_identifier;
}

/**
 * @deprecated — kept as a compatibility shim for older specs. New tests
 * should use {@link fetchBookUuidByTitle}, since `/books/:id` was
 * replaced by `/books/:uuid` for URL stability across reindexes.
 */
export const fetchBookIdByTitle = fetchBookUuidByTitle;

/** Locate the row for a fixture by its slug — matches `data-testid="ebook-row-${slug}"`. */
export function getRow(page: Page, slug: string): Locator {
  return page.getByTestId(`ebook-row-${slug}`);
}

/**
 * Switch the landing library to the table view. Grid is the default view
 * mode, so any spec that asserts against table rows/cells must opt in first.
 */
export async function switchToTableView(page: Page): Promise<void> {
  await page.getByTestId("view-toggle-table").click();
  await expect(page.getByTestId("ebook-table")).toBeVisible();
}

/** Expected text for the series cell, mirroring the Rust formatter:
 *  `${name} #${idx}` when both are present, just `${name}` when no index,
 *  empty string otherwise. */
function expectedSeriesText(book: ExpectedBook): string {
  if (book.series && book.seriesIndex)
    return `${book.series} #${book.seriesIndex}`;
  if (book.series) return book.series;
  return "";
}

/**
 * Assert every visible cell in a fixture's row matches the expected metadata.
 * Each per-cell testid (`ebook-cell-title`, `-author`, `-series`,
 * `-publisher`, `-published`, `-language`, `-cover`) is scoped under the row
 * locator so two books with the same e.g. publisher don't collide.
 */
export async function expectRowMatches(
  page: Page,
  expected: ExpectedBook,
): Promise<void> {
  const row = getRow(page, expected.slug);
  await expect(
    row,
    `row for slug "${expected.slug}" should be visible`,
  ).toBeVisible();

  await expect(row.getByTestId("ebook-cell-title")).toHaveText(expected.title);
  await expect(row.getByTestId("ebook-cell-author")).toHaveText(
    expected.authors.join(", "),
  );
  await expect(row.getByTestId("ebook-cell-series")).toHaveText(
    expectedSeriesText(expected),
  );
  await expect(row.getByTestId("ebook-cell-publisher")).toHaveText(
    expected.publisher ?? "",
  );
  await expect(row.getByTestId("ebook-cell-published")).toHaveText(
    expected.published ?? "",
  );
  await expect(row.getByTestId("ebook-cell-language")).toHaveText(
    expected.language,
  );

  const coverCell = row.getByTestId("ebook-cell-cover");
  if (expected.hasCover) {
    await expect(
      coverCell.getByRole("img", { name: `Cover of ${expected.title}` }),
    ).toBeVisible();
  } else {
    await expect(coverCell).toHaveText("—");
  }
}

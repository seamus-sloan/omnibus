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

const MONTH_NAMES = [
  "January",
  "February",
  "March",
  "April",
  "May",
  "June",
  "July",
  "August",
  "September",
  "October",
  "November",
  "December",
];

// Calibre's "no publish date" placeholder (`UNDEFINED_DATE =
// datetime(101, 1, 1, tzinfo=utc)`) and anything at or before it in year —
// mirrors `SENTINEL_YEAR_MAX` in `frontend/src/format.rs`.
const SENTINEL_YEAR_MAX = 101;

/**
 * Expected text for the published cell, mirroring
 * `frontend::format::format_date_short` (#1905): a stored `YYYY[-MM[-DD]]`
 * date renders as `"Month D, YYYY"`, narrowing to `"Month YYYY"` or `"YYYY"`
 * as the source narrows; an absent, unparsable, or sentinel date renders as
 * an em dash. Keep the two formatters in lockstep.
 */
function expectedPublishedText(raw: string): string {
  const datePart = raw.trim().split(/[T ]/)[0] ?? "";
  const [yearStr, monthStr, dayStr] = datePart.split("-");
  const year = Number(yearStr);
  if (!yearStr || !Number.isFinite(year) || year <= SENTINEL_YEAR_MAX) {
    return "—";
  }
  const month = monthStr ? Number(monthStr) : undefined;
  const dayNum = dayStr ? Number(dayStr) : undefined;
  // Explicit range check, not JS truthiness — `0` is a valid `Number` but an
  // invalid day, and falsy-zero would silently disagree with the Rust side's
  // `(1..=31).contains(d)` filter in `parse_date_prefix`.
  const day =
    dayNum !== undefined && dayNum >= 1 && dayNum <= 31 ? dayNum : undefined;
  const monthName =
    month && month >= 1 && month <= 12 ? MONTH_NAMES[month - 1] : undefined;
  if (monthName && day) return `${monthName} ${day}, ${year}`;
  if (monthName) return `${monthName} ${year}`;
  return `${year}`;
}

/**
 * Assert every visible cell in a fixture's row matches the expected metadata.
 * Each per-cell testid (`ebook-cell-title`, `-author`, `-series`,
 * `-tags`, `-published`, `-language`, `-cover`) is scoped under the row
 * locator so two books with the same e.g. series don't collide.
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
  // Tags are asserted only when the fixture pins them (`tags` present):
  // public-domain EPUBs carry long real-world subject lists that the db
  // parser tests already cover, and standalone-ocean's tags are mutated by
  // the inline-edit spec — both leave `tags` undefined to opt out.
  if (expected.tags) {
    await expect(row.getByTestId("ebook-cell-tags")).toHaveText(
      expected.tags.join(", "),
    );
  }
  await expect(row.getByTestId("ebook-cell-published")).toHaveText(
    expectedPublishedText(expected.published ?? ""),
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

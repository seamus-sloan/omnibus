import type { Locator, Page } from "@playwright/test";
import { expect } from "../fixtures/test";
import type { ExpectedBook } from "../fixtures/epubs";

/** Locate the row for a fixture by its slug — matches `data-testid="ebook-row-${slug}"`. */
export function getRow(page: Page, slug: string): Locator {
  return page.getByTestId(`ebook-row-${slug}`);
}

/** Expected text for the series cell, mirroring the Rust formatter:
 *  `${name} #${idx}` when both are present, just `${name}` when no index,
 *  empty string otherwise. */
function expectedSeriesText(book: ExpectedBook): string {
  if (book.series && book.seriesIndex) return `${book.series} #${book.seriesIndex}`;
  if (book.series) return book.series;
  return "";
}

/**
 * Assert every visible cell in a fixture's row matches the expected metadata.
 * Each per-cell testid (`ebook-cell-title`, `-author`, `-series`,
 * `-publisher`, `-published`, `-language`, `-cover`) is scoped under the row
 * locator so two books with the same e.g. publisher don't collide.
 */
export async function expectRowMatches(page: Page, expected: ExpectedBook): Promise<void> {
  const row = getRow(page, expected.slug);
  await expect(row, `row for slug "${expected.slug}" should be visible`).toBeVisible();

  await expect(row.getByTestId("ebook-cell-title")).toHaveText(expected.title);
  await expect(row.getByTestId("ebook-cell-author")).toHaveText(expected.authors.join(", "));
  await expect(row.getByTestId("ebook-cell-series")).toHaveText(expectedSeriesText(expected));
  await expect(row.getByTestId("ebook-cell-publisher")).toHaveText(expected.publisher ?? "");
  await expect(row.getByTestId("ebook-cell-published")).toHaveText(expected.published ?? "");
  await expect(row.getByTestId("ebook-cell-language")).toHaveText(expected.language);

  const coverCell = row.getByTestId("ebook-cell-cover");
  if (expected.hasCover) {
    await expect(coverCell.getByRole("img", { name: `Cover of ${expected.title}` })).toBeVisible();
  } else {
    await expect(coverCell).toHaveText("—");
  }
}

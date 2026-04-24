/**
 * Single source of truth for the synthetic EPUB fixtures the landing-page
 * spec asserts against. Every entry corresponds 1:1 with a file in
 * `test-data/epubs/generated/` produced by `tools/make_epub.ts`.
 *
 * The `slug` mirrors the Rust `row_slug()` derivation in
 * `frontend/src/pages/landing.rs` (filename stem, lowercased, runs of
 * non-alphanumerics → `-`). When you add or rename a fixture, update both
 * the generator inputs and this table, then regenerate the EPUBs.
 */
export interface ExpectedBook {
  /** `data-testid` suffix on the row — `ebook-row-${slug}`. */
  slug: string;
  /** On-disk filename relative to `test-data/epubs/`. */
  filename: string;
  /** Rendered in the title cell. */
  title: string;
  /** Rendered comma-separated in the author cell, in order. */
  authors: string[];
  /** Series name; combined with `seriesIndex` as `${name} #${idx}` if both set. */
  series?: string;
  seriesIndex?: string;
  /** Verbatim text rendered in the publisher cell. */
  publisher?: string;
  /** Verbatim text rendered in the published cell. */
  published?: string;
  /** Verbatim text rendered in the language cell (BCP-47). */
  language: string;
  /** True iff the EPUB ships an embedded cover image. */
  hasCover: boolean;
}

export const FIXTURE_BOOKS: readonly ExpectedBook[] = [
  {
    slug: "alpha",
    filename: "alpha.epub",
    title: "Alpha",
    authors: ["Ada Lovelace"],
    publisher: "Omnibus Test Press",
    published: "1843-10-01",
    language: "en",
    hasCover: true,
  },
  {
    slug: "beta",
    filename: "beta.epub",
    title: "Beta in the Series",
    authors: ["Grace Hopper", "Margaret Hamilton"],
    series: "Pioneers",
    seriesIndex: "1",
    publisher: "Omnibus Test Press",
    published: "1969-07-20",
    language: "en",
    hasCover: true,
  },
  {
    slug: "gamma",
    filename: "gamma.epub",
    title: "Gamma sin Cover",
    authors: ["Hedy Lamarr"],
    series: "Pioneers",
    seriesIndex: "2",
    publisher: "Editorial Omnibus",
    published: "1942-08-11",
    language: "es",
    hasCover: false,
  },
] as const;

/**
 * Single source of truth for the synthetic EPUB fixtures the landing-page
 * spec asserts against. Every entry corresponds 1:1 with a file in
 * `test_data/epubs/generated/` produced by `tools/make_epub.ts`.
 *
 * The `slug` mirrors the Rust `row_slug()` derivation in
 * `frontend/src/pages/landing.rs` (filename stem, lowercased, runs of
 * non-alphanumerics → `-`). When you add or rename a fixture, update both
 * the generator inputs and this table, then regenerate the EPUBs.
 */
export interface ExpectedBook {
  /** `data-testid` suffix on the row — `ebook-row-${slug}`. */
  slug: string;
  /** On-disk filename relative to `test_data/epubs/`. */
  filename: string;
  /** Rendered in the title cell. */
  title: string;
  /** Rendered comma-separated in the author cell, in order. */
  authors: string[];
  /** Series name; combined with `seriesIndex` as `${name} #${idx}` if both set. */
  series?: string;
  seriesIndex?: string;
  /**
   * The EPUB's `dc:publisher` metadata. No longer a table column (the
   * Tags column replaced Publisher); still shown on the book detail page.
   */
  publisher?: string;
  /**
   * Rendered comma-separated in the tags cell, sorted case-insensitively
   * (the DB projection orders subjects by tag name). Undefined skips the
   * cell assertion entirely — used by the public-domain fixtures (long
   * real-world subject lists, covered by `db/tests/public_domain_epubs.rs`)
   * and by `standalone-ocean` (its tags are mutated by the inline-edit
   * spec). The generator emits no `dc:subject`, so the other generated
   * fixtures pin `[]`.
   */
  tags?: string[];
  /**
   * Raw stored `dc:date` value (e.g. `"1843-10-01"`). The published cell
   * renders a formatted human date derived from this, not the raw string —
   * see `expectedPublishedText` in `utils/ebooks.ts`.
   */
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
    tags: [],
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
    tags: [],
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
    tags: [],
    language: "es",
    hasCover: false,
  },

  // Pioneers series, extended (beta=#1, gamma=#2 above).
  {
    slug: "pioneers-3",
    filename: "pioneers-3.epub",
    title: "Pioneers Vol 3: Cipher",
    authors: ["Joan Clarke"],
    series: "Pioneers",
    seriesIndex: "3",
    publisher: "Omnibus Test Press",
    published: "1947-06-12",
    tags: [],
    language: "en",
    hasCover: true,
  },
  {
    slug: "pioneers-4",
    filename: "pioneers-4.epub",
    title: "Pioneers Vol 4: Loop",
    authors: ["Karen Sparck Jones"],
    series: "Pioneers",
    seriesIndex: "4",
    publisher: "Omnibus Test Press",
    published: "1972-11-02",
    tags: [],
    language: "en",
    hasCover: true,
  },
  {
    slug: "pioneers-5",
    filename: "pioneers-5.epub",
    title: "Pioneers Vol 5: Signal",
    authors: ["Radia Perlman"],
    series: "Pioneers",
    seriesIndex: "5",
    publisher: "Omnibus Test Press",
    published: "1985-03-15",
    tags: [],
    language: "en",
    hasCover: true,
  },

  // Code Quartet (4 books).
  {
    slug: "code-quartet-1",
    filename: "code-quartet-1.epub",
    title: "Quartet I: Lexer",
    authors: ["Niklaus Wirth"],
    series: "Code Quartet",
    seriesIndex: "1",
    publisher: "Verlag Algorithmus",
    published: "1976-04-01",
    tags: [],
    language: "de",
    hasCover: true,
  },
  {
    slug: "code-quartet-2",
    filename: "code-quartet-2.epub",
    title: "Quartet II: Parser",
    authors: ["Niklaus Wirth"],
    series: "Code Quartet",
    seriesIndex: "2",
    publisher: "Verlag Algorithmus",
    published: "1977-08-12",
    tags: [],
    language: "de",
    hasCover: true,
  },
  {
    slug: "code-quartet-3",
    filename: "code-quartet-3.epub",
    title: "Quartet III: Type Checker",
    authors: ["Niklaus Wirth", "Per Brinch Hansen"],
    series: "Code Quartet",
    seriesIndex: "3",
    publisher: "Verlag Algorithmus",
    published: "1978-09-10",
    tags: [],
    language: "en",
    hasCover: false,
  },
  {
    slug: "code-quartet-4",
    filename: "code-quartet-4.epub",
    title: "Quartet IV: Codegen",
    authors: ["Niklaus Wirth"],
    series: "Code Quartet",
    seriesIndex: "4",
    publisher: "Verlag Algorithmus",
    published: "1979-12-01",
    tags: [],
    language: "en",
    hasCover: true,
  },

  // Polyglot Tales (3 books, three different languages).
  {
    slug: "polyglot-1",
    filename: "polyglot-1.epub",
    title: "Polyglot Tales: Recits",
    authors: ["Evariste Galois"],
    series: "Polyglot Tales",
    seriesIndex: "1",
    publisher: "Maison Polyglotte",
    published: "1830-05-29",
    tags: [],
    language: "fr",
    hasCover: true,
  },
  // Reserved for check_in_link_existing.spec.ts: it checks a real physical
  // copy in against this book. `physical_copies` rows are library-wide (they
  // add a PHYS badge to the formats cell and flip the detail page's physical
  // pill for every user), and the scanned ISBN then resolves to this book on
  // the exact-identifier rung forever after — so no other spec may read it,
  // even though the spec deletes the copy again on the way out.
  {
    slug: "polyglot-2",
    filename: "polyglot-2.epub",
    title: "Polyglot Tales: Cuentos",
    authors: ["Jorge Luis Borges"],
    series: "Polyglot Tales",
    seriesIndex: "2",
    publisher: "Maison Polyglotte",
    published: "1944-06-01",
    tags: [],
    language: "es",
    hasCover: true,
  },
  {
    slug: "polyglot-3",
    filename: "polyglot-3.epub",
    title: "Polyglot Tales: Monogatari",
    authors: ["Soseki Natsume"],
    series: "Polyglot Tales",
    seriesIndex: "3",
    publisher: "Maison Polyglotte",
    published: "1914-04-20",
    tags: [],
    language: "ja",
    hasCover: true,
  },

  // Compiler Compendium (3 books, all multi-author with a shared lead).
  {
    slug: "compiler-compendium-1",
    filename: "compiler-compendium-1.epub",
    title: "Compendium of Compilers I",
    authors: ["Alfred Aho", "Jeffrey Ullman"],
    series: "Compiler Compendium",
    seriesIndex: "1",
    publisher: "MIT Press Mirror",
    published: "1977-01-01",
    tags: [],
    language: "en",
    hasCover: true,
  },
  {
    slug: "compiler-compendium-2",
    filename: "compiler-compendium-2.epub",
    title: "Compendium of Compilers II",
    authors: ["Alfred Aho", "Monica Lam"],
    series: "Compiler Compendium",
    seriesIndex: "2",
    publisher: "MIT Press Mirror",
    published: "1986-10-15",
    tags: [],
    language: "en",
    hasCover: true,
  },
  {
    slug: "compiler-compendium-3",
    filename: "compiler-compendium-3.epub",
    title: "Compendium of Compilers III",
    authors: ["Alfred Aho", "Ravi Sethi", "Jeffrey Ullman"],
    series: "Compiler Compendium",
    seriesIndex: "3",
    publisher: "MIT Press Mirror",
    published: "2006-08-31",
    tags: [],
    language: "en",
    hasCover: false,
  },

  // Mathematica Minor (3 books, mixed languages, mixed cover availability).
  {
    slug: "mathematica-minor-1",
    filename: "mathematica-minor-1.epub",
    title: "Minor Lemmas I",
    authors: ["Emmy Noether"],
    series: "Mathematica Minor",
    seriesIndex: "1",
    publisher: "Klein Mathematik",
    published: "1918-07-23",
    tags: [],
    language: "de",
    hasCover: true,
  },
  {
    slug: "mathematica-minor-2",
    filename: "mathematica-minor-2.epub",
    title: "Minor Lemmas II",
    authors: ["Sophie Germain"],
    series: "Mathematica Minor",
    seriesIndex: "2",
    publisher: "Klein Mathematik",
    published: "1815-01-08",
    tags: [],
    language: "fr",
    hasCover: true,
  },
  {
    slug: "mathematica-minor-3",
    filename: "mathematica-minor-3.epub",
    title: "Minor Lemmas III",
    authors: ["Sofya Kovalevskaya"],
    series: "Mathematica Minor",
    seriesIndex: "3",
    publisher: "Klein Mathematik",
    published: "1888-12-24",
    tags: [],
    language: "en",
    hasCover: false,
  },

  // Standalones (9 unique-author books spanning languages/publishers).
  {
    slug: "standalone-island",
    filename: "standalone-island.epub",
    title: "The Isle of Functions",
    authors: ["Haskell Curry"],
    publisher: "Lambda Books",
    published: "1934-09-12",
    tags: [],
    language: "en",
    hasCover: true,
  },
  {
    slug: "standalone-garden",
    filename: "standalone-garden.epub",
    title: "The Garden of Closures",
    authors: ["Alonzo Church"],
    publisher: "Lambda Books",
    published: "1936-04-19",
    tags: [],
    language: "en",
    hasCover: true,
  },
  {
    slug: "standalone-river",
    filename: "standalone-river.epub",
    title: "Rio de Monadas",
    authors: ["Maria Zambrano"],
    publisher: "Editorial Omnibus",
    published: "1957-03-03",
    tags: [],
    language: "es",
    hasCover: false,
  },
  {
    slug: "standalone-mountain",
    filename: "standalone-mountain.epub",
    title: "Berg der Beweise",
    authors: ["David Hilbert"],
    publisher: "Klein Mathematik",
    published: "1900-08-08",
    tags: [],
    language: "de",
    hasCover: true,
  },
  {
    slug: "standalone-forest",
    filename: "standalone-forest.epub",
    title: "La Foret des Algorithmes",
    authors: ["Henri Poincare"],
    publisher: "Maison Polyglotte",
    published: "1899-11-17",
    tags: [],
    language: "fr",
    hasCover: true,
  },
  {
    // Reserved for landing_inline_edit.spec.ts's Tags-cell tests: they hold
    // a `subjects` override on this book mid-test, so `tags` stays
    // undefined (unasserted) and no other spec may touch its overrides.
    slug: "standalone-ocean",
    filename: "standalone-ocean.epub",
    title: "Ocean of Bytes",
    authors: ["Donald Knuth", "Leslie Lamport"],
    publisher: "MIT Press Mirror",
    published: "1989-06-21",
    language: "en",
    hasCover: true,
  },
  {
    slug: "standalone-desert",
    filename: "standalone-desert.epub",
    title: "Desert Protocols",
    authors: ["Vint Cerf", "Bob Kahn"],
    publisher: "Omnibus Test Press",
    published: "1974-05-10",
    tags: [],
    language: "en",
    hasCover: false,
  },
  // Reserved for the journal insert-from-highlights spec (`journal.spec.ts`):
  // it seeds a highlight on this book, and highlights are per-(user, book)
  // server state that survives the run, so any spec asserting an empty
  // highlights state on it would race that seed. No other spec may read it.
  {
    slug: "standalone-meadow",
    filename: "standalone-meadow.epub",
    title: "Meadow of Bits",
    authors: ["Claude Shannon"],
    publisher: "Omnibus Test Press",
    published: "1948-07-30",
    tags: [],
    language: "en",
    hasCover: true,
  },
  {
    slug: "standalone-canyon",
    filename: "standalone-canyon.epub",
    title: "Canyon Echoes",
    authors: ["Annie Easley"],
    publisher: "Lambda Books",
    published: "1955-04-23",
    tags: [],
    language: "en",
    hasCover: true,
  },
  // Reserved for genres.spec.ts. Genres live only in `metadata_overrides`,
  // so every genre assertion is a write against shared server state that
  // outlives the test — and it also flips `has_override` on the book. No
  // other spec may read or write this one.
  {
    slug: "standalone-tundra",
    filename: "standalone-tundra.epub",
    title: "Tundra Signals",
    authors: ["Mary Jackson"],
    publisher: "Omnibus Test Press",
    published: "1958-09-12",
    tags: [],
    language: "en",
    hasCover: true,
  },

  // Dual-format book: shares a normalized (title, author) with the
  // "Immersive Voyage" audiobook (`fixtures/audiobooks.ts`), so the indexer
  // auto-attaches the two into ONE book with both formats — the Immersive
  // Read precondition. See `fixtures/dual_format.ts`. The title sorts under
  // "i" (before "The Isle of Functions"), so it never displaces the
  // alphabetically-last fixture the landing sort test asserts on.
  {
    slug: "immersive-voyage",
    filename: "immersive-voyage.epub",
    title: "Immersive Voyage",
    authors: ["Alan Turing"],
    publisher: "Omnibus Test Press",
    published: "1950-10-01",
    tags: [],
    language: "en",
    hasCover: true,
  },

  // Second dual-format pair, RESERVED for cross_format_prompts.spec.ts —
  // that spec confirms a cross-format link and writes progress on this
  // book (globally visible server state), so no other spec may read it.
  {
    slug: "parallel-latitudes",
    filename: "parallel-latitudes.epub",
    title: "Parallel Latitudes",
    authors: ["Vera Molnar"],
    publisher: "Omnibus Test Press",
    published: "1968-05-01",
    tags: [],
    language: "en",
    hasCover: true,
  },

  // Reserved for landing_bulk_edit.spec.ts — it bulk-writes publisher/tag
  // overrides to BOTH books (reverted at test end, but the suite is
  // fullyParallel, so no other spec may read them). `tags` is deliberately
  // omitted (like standalone-ocean) so the landing spec skips the tags-cell
  // assertion these mutations would race. "Katherine Johnson" is unique
  // across ALL fixtures (ebook + audiobook) — shelves.spec.ts asserts exact
  // author-scoped match counts.
  {
    slug: "bulk-target-1",
    filename: "bulk-target-1.epub",
    title: "Bulk Target One",
    authors: ["Katherine Johnson"],
    publisher: "Bulk Test House",
    published: "1972-02-02",
    language: "en",
    hasCover: true,
  },
  {
    slug: "bulk-target-2",
    filename: "bulk-target-2.epub",
    title: "Bulk Target Two",
    authors: ["Katherine Johnson"],
    publisher: "Bulk Test House",
    published: "1974-03-03",
    language: "en",
    hasCover: true,
  },

  // The two CBZ fixtures (tools/make_cbz.ts) are reserved for the
  // comic-pager spec (comic_reader.spec.ts): `aurora-station-01` receives
  // that spec's progress and read-status writes (the pager auto-marks
  // reading on open and finished on the last page), and `aurora-station-02`
  // must stay progress-free — the layout test asserts it opens on page 1,
  // so no spec (including the pager spec's own write path) may save a
  // position on it. Their author appears in no other fixture. CBZ carries
  // no publisher/date/language metadata, hence the empty cells.
  {
    slug: "aurora-station-01",
    filename: "aurora-station-01.cbz",
    title: "Aurora Station, Vol. 1",
    authors: ["Sora Inkwell"],
    series: "Aurora Station",
    seriesIndex: "1",
    tags: [],
    language: "",
    hasCover: true,
  },
  {
    slug: "aurora-station-02",
    filename: "aurora-station-02.cbz",
    title: "Aurora Station, Vol. 2",
    authors: ["Sora Inkwell"],
    series: "Aurora Station",
    seriesIndex: "2",
    tags: [],
    language: "",
    hasCover: true,
  },

  // Public-domain Project Gutenberg / Standard Ebooks EPUBs under
  // `test_data/epubs/public_domain/`. Metadata below is what each file's OPF
  // actually claims — `db/tests/public_domain_epubs.rs` keeps the parser honest.
  //
  // `dracula` is reserved for the TOC-jump loading-state regression in
  // reader.spec.ts (issue #1909): it needs a real multi-chapter book so the
  // contents drawer lists more than one row to jump between — no other spec
  // may open it in the reader.
  {
    slug: "dracula",
    filename: "public_domain/dracula.epub",
    title: "Dracula",
    authors: ["Bram Stoker"],
    published: "1995-10-01",
    language: "en",
    hasCover: true,
  },
  // `frankenstein` and `great-gatsby` are reserved for the reader
  // progress-restore specs in reader.spec.ts (frankenstein: real progress
  // writes, great-gatsby: forced-failure path) — no other spec may open them
  // in the reader, since saved position is per-book state shared across
  // parallel workers. The progress specs need real multi-page books; the
  // synthetic one-paragraph fixtures render as a single page (no page turns,
  // no page numbers).
  {
    slug: "frankenstein",
    filename: "public_domain/frankenstein.epub",
    title: "Frankenstein; or, the modern prometheus",
    authors: ["Mary Wollstonecraft Shelley"],
    published: "1993-10-01",
    language: "en",
    hasCover: true,
  },
  {
    slug: "great-gatsby",
    filename: "public_domain/great_gatsby.epub",
    title: "The Great Gatsby",
    authors: ["F. Scott Fitzgerald"],
    published: "2021-01-17",
    language: "en",
    hasCover: true,
  },
  {
    slug: "romeo-and-juliet",
    filename: "public_domain/romeo_and_juliet.epub",
    title: "Romeo and Juliet",
    authors: ["William Shakespeare"],
    published: "1998-11-01",
    language: "en",
    hasCover: true,
  },

  // Additional Project Gutenberg EPUBs (17). Metadata below is what each
  // file's OPF actually claims — `db/tests/public_domain_epubs.rs` keeps
  // the parser honest. PG releases don't set publisher or series.
  {
    slug: "count-of-monte-cristo",
    filename: "public_domain/count_of_monte_cristo.epub",
    title: "The Count of Monte Cristo",
    authors: ["Alexandre Dumas", "Auguste Maquet"],
    published: "1998-01-01",
    language: "en",
    hasCover: true,
  },
  {
    slug: "pride-and-prejudice",
    filename: "public_domain/pride_and_prejudice.epub",
    title: "Pride and Prejudice",
    authors: ["Jane Austen"],
    published: "1998-06-01",
    language: "en",
    hasCover: true,
  },
  {
    slug: "middlemarch",
    filename: "public_domain/middlemarch.epub",
    title: "Middlemarch",
    authors: ["George Eliot"],
    published: "1994-07-01",
    language: "en",
    hasCover: true,
  },
  {
    slug: "picture-of-dorian-gray",
    filename: "public_domain/picture_of_dorian_gray.epub",
    title: "The Picture of Dorian Gray",
    authors: ["Oscar Wilde"],
    published: "1994-10-01",
    language: "en",
    hasCover: true,
  },
  {
    slug: "civics-and-health",
    filename: "public_domain/civics_and_health.epub",
    title: "Civics and Health",
    authors: ["William H. Allen", "W. T. Sedgwick"],
    published: "2007-05-08",
    language: "en",
    hasCover: true,
  },
  {
    slug: "tribes-and-castes-of-india",
    filename: "public_domain/tribes_and_castes_of_india.epub",
    title: "The Tribes and Castes of the Central Provinces of India, Volume 2",
    authors: ["R. V. Russell"],
    published: "2007-07-06",
    language: "en",
    hasCover: true,
  },
  {
    slug: "woordenlijst-nederlandsche-taal",
    filename: "public_domain/woordenlijst_nederlandsche_taal.epub",
    title:
      "Woordenlijst voor de spelling der Nederlandsche Taal / Met aanwijzing van de geslachten der naamwoorden en de vervoeging der werkwoorden",
    authors: ["M. de Vries", "L. A. te Winkel", "Adriaan Beets"],
    published: "2007-09-22",
    language: "nl",
    hasCover: true,
  },
  {
    slug: "sir-richard-calmady",
    filename: "public_domain/sir_richard_calmady.epub",
    title: "The History of Sir Richard Calmady: A Romance",
    authors: ["Lucas Malet"],
    published: "2007-12-09",
    language: "en",
    hasCover: true,
  },
  {
    slug: "mariucha",
    filename: "public_domain/mariucha.epub",
    title: "Mariucha",
    authors: ["Benito Pérez Galdós", "S. Griswold Morley"],
    published: "2008-03-25",
    // OPF claims `en`; the work is Spanish but we record what the file says.
    language: "en",
    hasCover: true,
  },
  {
    slug: "life-of-charles-dickens",
    filename: "public_domain/life_of_charles_dickens.epub",
    title: "The Life of Charles Dickens, Vol. I-III, Complete",
    authors: ["John Forster"],
    published: "2008-06-20",
    language: "en",
    hasCover: true,
  },
  {
    slug: "charles-frohman",
    filename: "public_domain/charles_frohman.epub",
    title: "Charles Frohman: Manager and Man",
    authors: ["Isaac Frederick Marcosson", "Daniel Frohman", "J. M. Barrie"],
    published: "2008-07-29",
    language: "en",
    hasCover: true,
  },
  {
    slug: "room-with-a-view",
    filename: "public_domain/room_with_a_view.epub",
    title: "A Room with a View",
    authors: ["E. M. Forster"],
    published: "2001-05-01",
    language: "en",
    hasCover: true,
  },
  {
    slug: "works-of-george-gillespie",
    filename: "public_domain/works_of_george_gillespie.epub",
    title: "The Works of Mr. George Gillespie (Vol. 1 of 2)",
    authors: ["George Gillespie", "W. M. Hetherington"],
    published: "2008-10-08",
    language: "en",
    hasCover: true,
  },
  {
    slug: "moby-dick",
    filename: "public_domain/moby_dick.epub",
    title: "Moby Dick; Or, The Whale",
    authors: ["Herman Melville"],
    published: "2001-07-01",
    language: "en",
    hasCover: true,
  },
  {
    slug: "little-women",
    filename: "public_domain/little_women.epub",
    title: "Little Women; Or, Meg, Jo, Beth, and Amy",
    authors: ["Louisa May Alcott", "Frank T. Merrill"],
    published: "2011-08-16",
    language: "en",
    hasCover: true,
  },
  {
    slug: "gogol-dramatische-werke",
    filename: "public_domain/gogol_dramatische_werke.epub",
    title: "Sämmtliche Werke 5: Dramatische Werke",
    authors: [
      "Nikolai Vasilevich Gogol",
      "Otto Buek",
      "Thomas Commichau",
      "Gregorius Itelson",
      "Alexandra Ramm",
      "Carl Ritter",
      "André Villard",
    ],
    published: "2017-09-05",
    language: "de",
    hasCover: true,
  },
  {
    slug: "wuthering-heights",
    filename: "public_domain/wuthering_heights.epub",
    title: "Wuthering Heights",
    authors: ["Emily Brontë"],
    published: "1996-12-01",
    language: "en",
    hasCover: true,
  },
] as const;

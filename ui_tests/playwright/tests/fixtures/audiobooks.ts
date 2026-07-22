/**
 * Single source of truth for audiobook fixtures the listen-page spec
 * asserts against. Covers both the synthetic generated fixtures
 * (`test_data/audiobooks/generated/`, produced by `tools/make_audiobook.ts`)
 * and the real public-domain LibriVox recordings
 * (`test_data/audiobooks/public_domain/`).
 *
 * The shape mirrors `tests/fixtures/epubs.ts::FIXTURE_BOOKS`: a flat,
 * data-only table the spec can iterate. When you add or rename a fixture,
 * update both the generator inputs and this table, then regenerate the
 * audio files.
 */
export interface ExpectedAudiobook {
  /** Book title (what the indexer surfaces as `EbookMetadata::title`).
   *
   * For single-file audiobooks the indexer uses TIT2; for multi-file
   * mp3-folder groups it uses TALB. The fixture inputs set TIT2 == TALB
   * for single-file books so this field always names the *book*. */
  title: string;
  /** Primary author — TPE1 on the first part. */
  author: string;
  /** File format badge (`MP3` / `M4B`). */
  format: "MP3" | "M4B";
  /** Number of parts the indexer should surface in `book_file_parts`. */
  parts: number;
  /** Whether the fixture has an embedded cover image. */
  hasCover: boolean;
  /** `"generated"` or `"public_domain"` — which subdirectory tree. */
  source: "generated" | "public_domain";
}

/**
 * Expected per-part duration for the *generated* fixtures. Derived from
 * `MP3_FRAME_DURATION_SEC` in `tools/make_audiobook.ts` (80 frames ×
 * 1152/44100 ≈ 2.09 s). Matches what `lofty` returns (~2.085 s).
 */
export const GENERATED_DURATION_SEC = 2.085;

export const AUDIOBOOK_BOOKS: readonly ExpectedAudiobook[] = [
  // --- Generated (synthetic silent MP3s with embedded 1×1 PNG covers) ---
  {
    title: "The Analytical Audiobook",
    author: "Ada Lovelace",
    format: "MP3",
    parts: 1,
    hasCover: true,
    source: "generated",
  },
  {
    title: "The Compiled Tales",
    author: "Grace Hopper",
    format: "MP3",
    parts: 2,
    hasCover: true,
    source: "generated",
  },

  // --- Public domain (real LibriVox recordings) ---
  {
    title: "A Song Of Long Ago",
    author: "James Whitcomb Riley",
    format: "MP3",
    parts: 3,
    hasCover: false,
    source: "public_domain",
  },
  {
    title: "A Woman's Love",
    author: "Sir Arthur Conan Doyle",
    format: "M4B",
    parts: 1,
    hasCover: false,
    source: "public_domain",
  },

  // Dual-format book: this single-file MP3 shares a normalized (title, author)
  // with the "Immersive Voyage" EPUB (`fixtures/epubs.ts`), so the indexer
  // auto-attaches them into ONE book with both formats — the Immersive Read
  // precondition (immersive.spec.ts). Appended last so the `.find(MP3 &&
  // generated)` selectors in listen/mini-dock/book_detail specs keep resolving
  // to "The Analytical Audiobook". See `fixtures/dual_format.ts`.
  {
    title: "Immersive Voyage",
    author: "Alan Turing",
    format: "MP3",
    parts: 1,
    hasCover: true,
    source: "generated",
  },

  // Merge-only pair — see MERGE_PRIMARY/MERGE_SECONDARY below. Appended last
  // for the same reason as "Immersive Voyage": the `.find(MP3 && generated)`
  // and `.find(MP3 && parts > 1)` selectors in listen/mini-dock/book_detail
  // keep resolving to "The Analytical Audiobook" and "The Compiled Tales".
  {
    title: "The Mergeable Manuscript",
    author: "Barbara Liskov",
    format: "MP3",
    parts: 1,
    hasCover: true,
    source: "generated",
  },
  {
    title: "The Severable Sequel",
    author: "Frances Allen",
    format: "MP3",
    parts: 1,
    hasCover: true,
    source: "generated",
  },
] as const;

/**
 * The audiobooks reserved for specs that MUTATE books via
 * `/api/rpc/merge-books` (`merge.spec.ts`, and `book_detail.spec.ts`'s
 * file-picker tests).
 *
 * The suite runs `fullyParallel` against one shared server, so a merge is
 * globally visible: while it is in flight the source book vanishes from
 * `/api/rpc/ebooks` and the target grows a second format and a second
 * `book_files` row. Pointing the merge specs at books that `listen.spec.ts`
 * / `mini-dock.spec.ts` also read is what made those specs fail in CI — the
 * `audiobook-merge` lock only serializes the *writers*, and readers never
 * took it. Keeping the mutated pair private to the writers removes the race
 * at the source instead of forcing ~27 reader tests through a mutex.
 *
 * **Never read these from a non-merge spec.** Use `AUDIOBOOK_BOOKS` for
 * anything that just needs an audiobook to look at.
 */
function requireAudiobook(title: string): ExpectedAudiobook {
  const found = AUDIOBOOK_BOOKS.find((b) => b.title === title);
  if (!found) {
    throw new Error(
      `merge-only audiobook fixture ${JSON.stringify(title)} is missing from ` +
        `AUDIOBOOK_BOOKS. The merge specs depend on it — re-add the entry and ` +
        `regenerate the MP3 via tools/make_audiobook.ts.`,
    );
  }
  return found;
}

export const MERGE_PRIMARY = requireAudiobook("The Mergeable Manuscript");
export const MERGE_SECONDARY = requireAudiobook("The Severable Sequel");

/**
 * Titles the reader specs must exclude when they pick "some generated MP3".
 * Without this the `.find(MP3 && generated)` selectors would silently start
 * reading a mutating fixture if the table order ever changed.
 */
export const MERGE_ONLY_TITLES: readonly string[] = [
  MERGE_PRIMARY.title,
  MERGE_SECONDARY.title,
];

/** Total audiobook *books* (groups) — not parts — the indexer surfaces. */
export const AUDIOBOOK_BOOK_COUNT = AUDIOBOOK_BOOKS.length;

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
] as const;

/** Total audiobook *books* (groups) — not parts — the indexer surfaces. */
export const AUDIOBOOK_BOOK_COUNT = AUDIOBOOK_BOOKS.length;

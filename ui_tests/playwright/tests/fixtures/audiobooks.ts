/**
 * Single source of truth for the synthetic audiobook fixtures the listen
 * page spec asserts against. Every entry corresponds 1:1 with a file (or
 * a folder of files) under `test_data/audiobooks/generated/` produced by
 * `tools/make_audiobook.ts`.
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
   * for single-file books and TIT2 != TALB for chapter parts so this
   * one field always names the *book*, not a chapter.
   */
  title: string;
  /** Primary author — TPE1 on the first part. Maps to the book's first
   *  `creators` row, which the listen page renders as `"by ${author}"`. */
  author: string;
  /** True when the on-disk layout is a folder of per-chapter MP3s
   *  (multi-part audiobook). False for a single-file `.mp3` group. */
  multipart: boolean;
  /** Expected number of parts the indexer should surface in
   *  `book_file_parts` for this group. */
  parts: number;
  /** Expected per-part duration in seconds. The fixtures all use the
   *  same generator params (80 silent MPEG-1 Layer III frames at
   *  44.1 kHz mono) so this is a single number rather than a per-part
   *  array. `lofty` rounds to ~2.085s vs the 2.09s the generator
   *  computes; both spec and Rust code accept the lofty-side value. */
  perPartDurationSec: number;
}

/**
 * Expected per-part duration in seconds for every fixture. Derived from
 * the MP3_FRAME_DURATION_SEC constant in `tools/make_audiobook.ts` and
 * pinned here as a literal so the spec doesn't pull the generator in.
 *
 * Matches what `lofty::file::AudioFile::properties().duration()` returns
 * for the generated files — verified by hand against
 * `lofty::read_from_path` on the committed fixtures.
 */
export const FIXTURE_DURATION_SEC = 2.085;

export const AUDIOBOOK_BOOKS: readonly ExpectedAudiobook[] = [
  {
    title: "The Analytical Audiobook",
    author: "Ada Lovelace",
    multipart: false,
    parts: 1,
    perPartDurationSec: FIXTURE_DURATION_SEC,
  },
  {
    title: "The Compiled Tales",
    author: "Grace Hopper",
    multipart: true,
    parts: 2,
    perPartDurationSec: FIXTURE_DURATION_SEC,
  },
] as const;

/** Total number of audiobook *books* (groups) — not parts — the indexer
 *  surfaces from the fixture library. Used by the seed helper to know
 *  when polling should stop. */
export const AUDIOBOOK_BOOK_COUNT = AUDIOBOOK_BOOKS.length;

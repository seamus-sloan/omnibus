/**
 * Single source of truth for audiobook fixtures the listen-page spec seeds
 * against. Each entry corresponds to one audiobook in
 * `test_data/audiobooks/public_domain/`.
 *
 * The indexer groups files by directory (mp3 folders) or treats single m4b
 * files as whole books — see `db::audiobook::group` for the rules.
 */
export interface ExpectedAudiobook {
  /** Human title as the indexer will extract it from tags. */
  title: string;
  /** Primary artist / author from the metadata. */
  artist: string;
  /** File format badge the landing page shows (`MP3` / `M4B`). */
  format: "MP3" | "M4B";
  /** Number of parts (tracks / files) in the audiobook. */
  partCount: number;
}

/**
 * Audiobook fixtures produced by `tools/make_audiobook.sh` from public
 * domain LibriVox recordings. Two books cover both supported layout
 * patterns:
 *
 * - MP3 folder: `James Whitcomb Riley/A Song of Long Ago/` (3 tracks)
 * - Single M4B: `Arthur Conan Doyle/A Womans Love.m4b` (1 file)
 */
export const FIXTURE_AUDIOBOOKS: readonly ExpectedAudiobook[] = [
  {
    title: "A Song Of Long Ago",
    artist: "James Whitcomb Riley",
    format: "MP3",
    partCount: 3,
  },
  {
    title: "A Woman's Love",
    artist: "Sir Arthur Conan Doyle",
    format: "M4B",
    partCount: 1,
  },
];

/** Total number of audiobook entities the indexer should surface. */
export const EXPECTED_AUDIOBOOK_COUNT = FIXTURE_AUDIOBOOKS.length;

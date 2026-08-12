/**
 * The one fixture book that exists as BOTH an EPUB and an audiobook.
 *
 * `make_epub.ts` and `make_audiobook.ts` each emit a file whose (title,
 * author) is byte-identical to the other. The indexer normalizes (title,
 * author) the same way across the ebook and audiobook libraries and
 * auto-attaches the pair (`db::sync::attach::find_attach_target`) into a
 * single `books` row carrying both formats — the precondition for the
 * Immersive Read CTA (`immersive.spec.ts`). Because the display strings
 * match exactly, the attached book reads the same regardless of which
 * library the indexer walks first.
 *
 * `RESUME_PROMPT_BOOK` is the second intentional pair, reserved for
 * `cross_format_prompts.spec.ts`; every other EPUB/audiobook pair is
 * deliberately distinct so the combined library count stays additive.
 */
export const DUAL_FORMAT_BOOK = {
  title: "Immersive Voyage",
  author: "Alan Turing",
  /** `data-testid` suffix on the landing row — `ebook-row-${slug}`. */
  slug: "immersive-voyage",
} as const;

/**
 * Number of (EPUB, audiobook) fixture pairs that share a normalized
 * (title, author) and therefore auto-attach into ONE combined book. A spec
 * that seeds BOTH libraries and asserts an exact combined total must
 * subtract this from the naive `FIXTURE_BOOKS.length + AUDIOBOOK_BOOK_COUNT`
 * sum — each attached pair is two files but only one book.
 */
export const AUTO_ATTACHED_PAIRS = 2;

/**
 * The dual-format pair RESERVED for `cross_format_prompts.spec.ts` — that
 * spec confirms a cross-format link and writes reading/listening progress
 * on it, all globally visible server state. No other spec may read it.
 */
export const RESUME_PROMPT_BOOK = {
  title: "Parallel Latitudes",
  author: "Vera Molnar",
  slug: "parallel-latitudes",
} as const;

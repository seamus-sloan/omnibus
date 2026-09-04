# 11 — Reporting where a reader is, and what they may see

Two questions the API answers about a reader mid-book: *where are you?* and
*may I show you this?* Both have one failure mode, and it is not being wrong
— it is being **confidently** wrong. Every rule here exists because a caller
that cannot tell "we know" from "we guessed" presents a guess as fact.

## Position is resolved server-side, once

A stored position is a CFI, a page anchor, or a number of seconds. None of
those is an answer to "what chapter am I on?", and every client that tried to
derive one did the same spine arithmetic by hand and got a different answer.

`db::progress::detail` resolves it instead, into `ResolvedPosition` —
`spine_index`, `chapter_title`, `chapter_ordinal`, `chapters_total`,
`percent_through_chapter`, `percent_through_book`.

- **Derived on the read path, never stored.** It comes from the structure
  tables (`epub_spine_stats` / `ebook_chapters` for reading, `file_chapters` /
  `book_file_parts` for listening) — the same rule 09 applies to content
  validators, for the same reason: a stored copy is a second fact some write
  path will forget to bump.
- **Every record carries the block**, including one that resolved to nothing.
  Omitting it makes "unresolvable" and "not asked for" the same shape.
- **`confidence` is part of the answer.** `Exact` means a real anchor resolved
  against extracted structure; `Approximate` means a lossy step (a stored
  integer percent mapped back onto the spine, or audio with no chapter marks);
  `Unknown` means nothing was derivable — and its sibling fields are then
  absent, which a caller **must not** read as position zero.
- **The weakest link wins.** An exact CFI inside a spine document holding
  several TOC entries is still `Approximate`: `ebook_chapters.start_chars` is
  recorded at spine granularity, so the data cannot say which of them the
  reader is in. Report the chapter the document opens with, and say so.

## Never name a chapter that is not one

`sync::insert_chapters` writes one `"Part N"` row per part when a container
carries no marks of its own. Those are not chapters. A 65-chapter book stored
as a 4-part M4B has four of them, and calling that "chapter 4 of 4" tells a
reader they have finished a book they are a quarter through.

- `detail::is_synthetic` detects that fallback by matching its own output
  exactly; a synthetic set reports the percent and **no** chapter vocabulary.
- The raw container figures travel as `audio_part` / `audio_part_count`, never
  `chapter_*`. Any surface rendering them says "Part", and reaches for
  "Chapter" only when `resolved` names one.

## Furthest means furthest, not latest

`BookProgress::furthest` ranks by distance through the book, ties broken by
event time. A reader who listened to 87% and then opened the EPUB at 47% to
check a name has not un-read the 87%, and a per-format read that defaults to
one format answers a question nobody asked.

## "Can't tell" is not "safe"

The spoiler boundary is the reader's own furthest position, so everything
above decides what may be shown. Three-valued throughout, exactly as rule 09
requires of the download validators:

- `ahead_of_reader: null` means **unknown**. A renderer may show the hit; a
  filter must not treat it as behind the reader.
- `SpoilerFilter::Exclude` withholds every hit it cannot *prove* is behind the
  reader, and reports `withheld_ahead` so a caller can say "there is an answer
  ahead of you" without having seen it.
- `?stop_at_progress=true` withholds the **whole** chapter when the reader's
  position cannot be placed. Handing over a chapter because the server could
  not work out where they were is the one outcome the parameter exists to
  prevent.
- A cut at the reader's position is **not** `truncated`. That flag invites a
  caller to page onward with `next_offset`; `truncated_by_progress` carries no
  cursor, because there is nowhere it would be safe to go.

Hits are placed at their chapter's **start** — the finest granularity the
content index records — which is conservative in the right direction: a
chapter the reader is partway through counts as behind them, and the risk
inside a chapter they are already reading is one they took themselves.

## Out of scope

- Which calendar a *day* is cut on — [10-reader-calendar.md](10-reader-calendar.md).
- Whether a downloaded file is stale — [09-content-validators.md](09-content-validators.md).

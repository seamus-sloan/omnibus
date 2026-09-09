# 11 — What a reader may be shown

The content tools answer one question with a safety property attached: *may I
show you this?* The failure mode is not being wrong — it is being
**confidently** wrong. Every rule here exists because a caller that cannot
tell "we know" from "we guessed" presents a guess as fact.

The boundary is the reader's own furthest position, read from the stored
percent columns. Where that figure comes from, and how much to trust it, is
`ResolvedPosition` — see the rustdoc on `db::progress::enrich`.

## "Can't tell" is not "safe"

Three-valued throughout, exactly as
[09-content-validators.md](09-content-validators.md) requires of the download
validators, and for the same reason:

- `ahead_of_reader: null` means **unknown**. A renderer may show the hit; a
  filter must not treat it as behind the reader.
- `SpoilerFilter::Exclude` withholds every hit it cannot *prove* is behind the
  reader — `!= Some(false)`, never `== Some(true)` — and reports
  `withheld_ahead` so a caller can say "there is an answer ahead of you"
  without having seen it.
- `?stop_at_progress=true` withholds the **whole** chapter when the reader's
  position cannot be placed. Handing over a chapter because the server could
  not work out where they were is the one outcome the parameter exists to
  prevent.
- A cut at the reader's position is **not** `truncated`. That flag invites a
  caller to page onward with `next_offset`; `truncated_by_progress` carries no
  cursor, because there is nowhere it would be safe to go.

## Round in the conservative direction

Hits are placed at their chapter's **start** — the finest granularity the
content index records — so a chapter the reader is partway through counts as
behind them. The risk inside a chapter they are already reading is one they
took themselves.

The same asymmetry governs every figure the boundary is derived from, and it
is why `progress::enrich` flooring its percents is load-bearing rather than
incidental: a floored percent never overstates how far the reader reached, so
a ceiling derived from it lands at or before their true place. Cutting a few
words early is harmless; cutting late is the spoiler all of this exists to
prevent. **A change that rounds any of these figures up is a spoiler bug**,
however small the rounding.

## The boundary is cheap on purpose

`content_fts::reader_position_percent` reads the stored `progress_percent` and
`audio_position_seconds` columns directly rather than calling
`progress::book_progress`, which opens the book and walks a spine document.
The boundary runs once per distinct book in a hit list of up to 50, on the
default search path. Sub-percent precision buys nothing against a hit placed
at its chapter's start.

A reading row with no stored percent yields `None` rather than a guess, and
`Exclude` withholds on it.

## Out of scope

- Which calendar a *day* is cut on — [10-reader-calendar.md](10-reader-calendar.md).
- Whether a downloaded file is stale — [09-content-validators.md](09-content-validators.md).

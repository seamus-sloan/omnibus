# Browsing authors

| | |
|---|---|
| **Weight** | 8% |
| **Owner-only** | no — but **never delete an author** |
| **Surfaces** | web, iOS |
| **Actions** | `authors.index`, `author.view`, `nav.follow` |

Walk the author index and open a few. This flow is mostly about whether the
index agrees with the library — author grouping is derived, and derived data is
where duplicates and mis-groupings hide.

## Steps

1. Open the list of authors.
2. Scan it. Look for the same person appearing twice under slightly different
   spellings, entries that are obviously not a person, and empty names.
3. Open an author with several books.
4. Check the books listed are actually by that author, and that the count
   matches what is shown.
5. Open one of them and confirm its detail page names the same author.
6. Go back and open a second author, ideally one with a single book.
7. Occasionally follow an author from a book's detail page rather than from the
   index, and confirm you land on the same page.

## Journal

`authors.index` with the total count and any suspicious entries verbatim —
near-duplicate spellings especially, since those are the finding. `author.view`
with the name, the book count shown, and the count you actually saw.

## Pass

- The index loads completely and is ordered sensibly.
- Each author page lists only that author's books.
- Counts match the books displayed.
- Reaching an author from a book and from the index gives the same page.

## Fail

- An author page listing books by someone else.
- A count that disagrees with the books shown.
- The same author split across two entries with identical spelling.
- An author page that errors or never loads.
- A book's detail page naming an author who has no entry in the index.

## Sharp edges

- **Do not delete an author**, whatever the page offers. It is destructive,
  library-wide, and not yours.
- Genuinely different spellings — an initial versus a full name, an accent
  present or absent — are a real and known messiness in library metadata.
  Journal them as `uncertain` rather than `fail`; they are worth collecting but
  are usually the source files, not the app.
- A book with several authors legitimately appears under each.
- Another agent may be editing an author's books as you read. Counts shifting
  between two visits is expected.

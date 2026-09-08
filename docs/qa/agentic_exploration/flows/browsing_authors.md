# Browsing authors

| | |
|---|---|
| **Runs** | on its own |
| **Owner-only** | no — but **never delete an author** |
| **Surfaces** | web, iOS |
| **Actions** | `author.index`, `author.view`, `nav.follow` |

Walk the author index and open a few. This flow is mostly about whether the
index agrees with the library — author grouping is derived, and derived data is
where duplicates and mis-groupings hide.

## Steps

1. Open the list of authors. Try its sort control (last name / most books)
   and its name filter, and confirm the header count while filtered — it may
   read as the library total rather than a match count; journal which.
2. Scan it. Look for the same person appearing twice under slightly different
   spellings, entries that are obviously not a person, and empty names.
3. Open an author with several books.
4. Check the books listed are actually by that author, and that the count
   matches what is shown.
5. Open one of them and confirm its detail page names the same author.
6. Go back and open a second author with a single book — the common case in
   a small library; the several-book author in step 3 is the scarce pick.
7. Occasionally follow an author from a book's detail page rather than from the
   index, and confirm you land on the same page.

## Journal

`author.index` (singular — the audit knows the noun `author`, not `authors`)
with the total count and any suspicious entries verbatim —
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
- A book with several authors legitimately appears under each. On iOS the
  detail page shows one author card for all of them and it opens only the
  first; journal that as an observation about the surface.
- A paper-only or wishlist book with no files counts on an author's page but
  may not in the index; journal both figures and say which surfaces agree.
- A stray credit that traces to a merge, an undo or a metadata edit belongs
  to that flow's journal, not this one's — the index renders stored credits
  faithfully.
- Another agent may be editing an author's books as you read. Counts shifting
  between two visits is expected.

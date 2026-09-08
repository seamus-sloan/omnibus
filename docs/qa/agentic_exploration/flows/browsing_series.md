# Browsing series

| | |
|---|---|
| **Runs** | on its own |
| **Owner-only** | no |
| **Surfaces** | web, iOS |
| **Actions** | `series.index`, `series.view`, `book.open`, `nav.follow` |

Walk the series index and open a few. Like authors, series grouping is derived
— but unlike authors it carries an **order**, and a wrong order is the failure
worth hunting here.

## Steps

1. Open the list of series. Try both sorts (A–Z and most books) and the
   name/author filter, including a filter that matches nothing. The header
   count stays at the library total while filtered — that is how it is
   built, not a finding — and the sort should survive a reload.
2. Scan it for duplicates, empty names, and series with an implausible number
   of entries.
3. Open a series with several books.
4. Check the books are ordered by their number in the series, and that the
   numbers shown are the numbers on the books themselves.
5. Look for gaps — book 1 and book 3 with no book 2, or, the common shape in
   a small library, a single owned entry numbered above 1 — and journal them.
   A gap usually means you do not own the missing books, which is fine, but
   it should be presented as a gap rather than silently renumbered.
6. Open a book from the series (journal it as `book.open`) and confirm its
   detail page agrees about the series name and its position in it — in the
   header eyebrow **and** in the MORE section's series block, which carries
   its own count, sibling list and "series page" link. The two have
   disagreed before.
7. For at least one book, reach its series from the detail page's series link
   and confirm it is the same page the index reaches.

## Journal

`series.index` with the total and anything odd. `series.view` with the series
name, the entries in the order shown, and each one's number.

## Pass

- The index loads and is ordered sensibly.
- A series page lists only that series' books.
- Books are in series order, with numbers matching their detail pages.
- Gaps are presented as gaps, not closed over.
- Reaching the series from a book and from the index gives the same page.

## Fail

- Books out of series order.
- A number on the series page disagreeing with the book's own detail page.
- A book appearing in a series it does not belong to.
- The same series split into two entries with identical names.
- A series page that errors or never loads.

## Sharp edges

- Not every book is in a series, and not every series book has a number. An
  unnumbered book sorted to the end is reasonable; journal it as `uncertain` if
  the placement looks arbitrary.
- Decimal numbers — a 1.5 novella between 1 and 2 — are legitimate. Sorting one
  correctly is a pass worth noting; sorting it to the end is a finding. A
  library may hold neither a decimal nor an unnumbered entry; journal those
  criteria as not exercisable rather than `uncertain`.
- A series page may show publisher blurbs with stray `*` characters — source
  markdown, not the app.
- Another agent may be editing series metadata as you read it, since that is a
  free-for-all edit. A name or number changing between visits is expected.

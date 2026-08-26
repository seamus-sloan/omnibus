# Browsing series

| | |
|---|---|
| **Weight** | 8% |
| **Owner-only** | no |
| **Surfaces** | web, iOS |
| **Actions** | `series.index`, `series.view`, `nav.follow` |

Walk the series index and open a few. Like authors, series grouping is derived
— but unlike authors it carries an **order**, and a wrong order is the failure
worth hunting here.

## Steps

1. Open the list of series.
2. Scan it for duplicates, empty names, and series with an implausible number
   of entries.
3. Open a series with several books.
4. Check the books are ordered by their number in the series, and that the
   numbers shown are the numbers on the books themselves.
5. Look for gaps — book 1 and book 3 with no book 2 — and journal them. A gap
   usually means you do not own book 2, which is fine, but it should be
   presented as a gap rather than silently renumbered.
6. Open a book from the series and confirm its detail page agrees about the
   series name and its position in it.
7. Occasionally reach a series from a book's detail page and confirm it is the
   same page.

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
  correctly is a pass worth noting; sorting it to the end is a finding.
- Another agent may be editing series metadata as you read it, since that is a
  free-for-all edit. A name or number changing between visits is expected.

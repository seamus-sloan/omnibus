# Reading a book

| | |
|---|---|
| **Weight** | 20% |
| **Owner-only** | no |
| **Surfaces** | web, iOS |
| **Actions** | `book.open`, `reader.progress`, `reader.close` |

Read roughly a tenth of a book, the way someone would on a lunch break. This is
the highest-weight flow because reading position is the single most-written
piece of state in the app, and the one whose loss a reader notices first.

## Preconditions

A book with an EPUB or CBZ format. Any book — you do not need to own it.
Prefer one you have read before if you have; resuming is more interesting than
starting.

## Steps

1. Reach the book from the library, an author page, a search, or the continue
   surface on the home page. Vary this between runs.
2. Open it to read. Note where it opened — at the start, or where you left off.
3. Read forward through roughly 10% of the book. Turn pages the way a person
   does: some quickly, some slowly. Do not spam the page-forward control.
   **If page-turning itself is broken**, that is a `fail` and you should journal
   it as one — but do not abandon the flow. Reaching a position through the
   table of contents, or through Resume, counts as arriving there, so carry on
   with the rest of the steps and say in the journal how you moved.
4. Somewhere in the middle, do one incidental thing a reader does — open the
   table of contents, change the font size, search for a word, switch the
   theme. Pick a different one each time.
5. Leave the reader deliberately, through the app's own way out.
6. Come back to the book's detail page and check that your position is
   reflected there.

## Journal

`book.open` with the uuid, the format, and where it opened. A `reader.progress`
entry each time you settle at a new position, carrying a **human-readable
location** (`"chapter 4, first paragraph"`) as well as whatever the app shows —
the audit needs to recognise the position later, and a raw CFI alone is not
something a person can check. `reader.close` with the final position.

Record the position you *left at* explicitly. That is the claim the audit tests.

## Pass

- The book opened where you expected — at the beginning if new to you, at your
  last position if not.
- Pages turned without blank frames, duplicated text, or content from a
  different chapter.
- Progress shown on the detail page afterwards matches roughly where you
  stopped.
- Re-opening the book returns you to that position.

## Fail

- It opened at the beginning of a book you were part-way through.
- Progress went backwards, or jumped somewhere you never were.
- A page rendered blank and stayed blank after a reload.
- Leaving and returning lost the position entirely.
- The reader hung, or the page-turn control stopped responding.

## Sharp edges

- **Opening the book set its read status to reading.** Expected. You did not do
  it and it is not a finding.
- **Reaching the end marks it finished**, which also drops it off the home
  page's continue surface. Both are correct.
- Progress is saved as you go, not on exit — so a position that persists after
  a hard close is correct behaviour, not a bug.
- A comic (CBZ) opens in a different reader from an EPUB. Both are in scope;
  say which you used.

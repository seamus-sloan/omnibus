# Resuming from another device

| | |
|---|---|
| **Runs** | inside reading_a_book |
| **Owner-only** | no |
| **Surfaces** | web, iOS |
| **Actions** | `book.open`, `reader.progress`, `progress.set` (runner) |

Somebody read this book on another device since you last saw it. The app must
put you where they left off — or, if that other device's position is older
than yours, leave you alone. Nothing in the run has a second device, so the
**runner plays one**: before handing you this flow it writes a position to
your account the way a phone or a Kobo would, and tells you what it wrote.

Runs inside [reading_a_book](reading_a_book.md): the book is the one you are
about to read, and the phantom position is set before you open it.

## What the runner hands you

Alongside the flow, three facts:

- **Which book** (uuid and title), the one drawn for the parent reading flow.
- **The variant**: `newer` or `stale`.
- **Where the phantom device left off**, in human terms — "about 40%, chapter
  6" — and whether it also marked the book **Reading**.

Under `newer`, the phantom position is ahead of anywhere you have been and is
stamped as the most recent event, so the app must resume there. Under
`stale`, the phantom position is behind where you already are and stamped
*earlier* than your own last write, so the app must ignore it and resume at
yours. The runner picks `stale` only for a book you have read before.

## Steps

1. **Before opening the reader**, look at the book from the outside: the
   continue surface on the home page, and the detail page's progress. Journal
   what each shows. Under `newer` they should already show the phantom
   position; under `stale` they should show yours.
2. Open the book to read. Note exactly where it opened.
3. Compare against what the runner told you. Under `newer`, you should be at
   the phantom position, give or take a page; under `stale`, at your own.
4. Read on for a few pages, past the phantom position if you are near it.
5. Leave the reader, come back to the detail page, and confirm the position
   shown is now **yours** — the phantom is superseded by your reading, not
   restored over it.
6. Reopen the reader once more and confirm it resumes at your latest
   position, not the phantom's.

## Journal

`book.open` with the uuid, the variant, the position the runner said the
phantom left, and the position the reader actually opened at — both in human
terms. `reader.progress` as you read, exactly as
[reading_a_book.md](reading_a_book.md) prescribes. The runner's own write is
already in the journal as `progress.set` under your actor with `surface`
`phantom`; do not write it again.

## Pass

- `newer`: the continue surface, the detail page, and the reader all agree on
  the phantom position before you read.
- `stale`: none of them moved; you resume at your own position.
- After you read past it, every surface shows your position and the reader
  resumes there.
- Read status is coherent: a phantom that marked the book Reading is reflected
  on the detail page.

## Fail

- `newer`: the reader opens at the beginning, or at an old position of yours.
- `stale`: the phantom position overrides yours.
- The surfaces disagree — the continue card says one place and the reader
  opens at another.
- Your reading is later overwritten by the phantom position. High severity:
  that is a device fighting the server and winning wrongly.
- Progress goes backwards on the detail page after you read forward.

## Sharp edges

- **A percent-only position is exact to a page, not a sentence.** The phantom
  writes a whole-book percent the way a Kobo does, so the reader may open at
  the start of the page containing it. Within a page is agreement.
- **Position and read status are separate writes.** A phantom position on a
  book you never opened does not by itself mark it Reading; the runner says
  whether it set the status too.
- The phantom position is stored against the **ebook** axis. If the book also
  has an audiobook, the listening position is untouched, which is correct.

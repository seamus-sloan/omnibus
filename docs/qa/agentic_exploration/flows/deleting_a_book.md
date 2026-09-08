# Deleting a book you added

| | |
|---|---|
| **Runs** | inside adding_book |
| **Owner-only** | **yes** |
| **Surfaces** | web |
| **Actions** | `book.delete`, `book.delete.verify` |

Remove a book's files. This is the most destructive thing an agent does, which
is why it runs only inside [adding_book](adding_book.md): the book you delete
is one you uploaded in this flow, moments ago, and nothing else in the run has
touched it.

**Only ever delete a book you added**, and prefer one you added in *this* run.
A book you uploaded in an earlier run is yours too, but another reader may
have rated it, highlighted it, or filed a copy against it since — deleting it
destroys their data, and the run's audit cannot tell that apart from a
loss. The guard refuses any book you do not own with a `403` carrying
`ownership_guard`; journal that `refused` and stop.

## Preconditions

A book you uploaded in this flow, confirmed by your own `book.add` entry with
its uuid. If the upload failed or attached to an existing book as a second
format, **do not delete** — the survivor is not yours alone. Journal that you
skipped and why.

## Steps

1. Open the book's detail page and journal its full state: uuid, title,
   author, formats, and whether anything of yours is on it.
2. Find **Delete files…** on the detail page. It is admin-only on the web and
   absent on iOS; if you are running as a non-admin reader it will not be
   there, which is correct — journal `refused` and end.
3. Read what the dialog offers. A book with two formats should let you choose
   which files go; a single-format book removes the only one. Journal the
   choice offered.
4. If the book has two formats, delete **one** first. Confirm the book
   survives with the other format, its detail page still opens, and the
   deleted format's reader or player is no longer offered.
5. Delete the remaining files.
6. Go back to the library and search for the book. Note **which** of two
   outcomes you see, because both may be intended: the book gone entirely, or
   the book still listed as an entry with no files. Journal which, and do not
   report either as a defect on its own.
7. Check the author and series pages the book was on. An author with no other
   books may vanish from the index; that is derived data behaving correctly.
8. Reload and confirm the state holds.

## Journal

`book.delete` with the uuid, the files chosen, and the state before.
`book.delete.verify` with what the library, the author page, and search show
afterwards.

**The audit does not verify deletions.** `book.delete` is listed as out of
scope because a deletion is library-wide. What the audit *will* notice is
your `book.add` for this book now failing its "present in the library" check —
so say in the `book.delete` entry that it supersedes the add, and the runner
will read the pair together.

## Pass

- The dialog names the book and files you chose.
- Deleting one format leaves the other intact and readable.
- After the final delete the book is gone or fileless, consistently across the
  library, search, and author pages.
- Nothing else changed — no other book lost a file, no author lost a book
  that was not this one.

## Fail

- Files from a **different book** are removed. High severity.
- The book disappears from one surface and persists on another after a
  reload.
- Deleting one format removes both.
- The dialog offers to delete a book you do not own without the guard
  refusing — that is a harness fault; journal it `high` and stop.
- The delete errors and leaves the book half-present.

## Sharp edges

- The book's uuid may still resolve afterwards, to a fileless entry or a
  redirect. Old links keeping some meaning is deliberate.
- Thumbnails and covers are cached. A cover lingering in the grid for a
  moment after the book is gone is not a defect.
- A book you deleted is still in your journal as owned. That is correct — the
  ledger records what you added, not what still exists.

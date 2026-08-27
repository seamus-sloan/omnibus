# Merging two books

| | |
|---|---|
| **Weight** | 50% of an add-a-book flow |
| **Owner-only** | **yes — both books** |
| **Surfaces** | web |
| **Actions** | `merge.attempt`, `merge.confirm`, `merge.undo` |

Merge combines two library entries into one, typically an ebook and an
audiobook of the same work. It is destructive and irreversible-looking, so it
is owner-only on **both** sides: you may merge two books only if your journal
shows a `book.add` from you for each of them.

Never merge a book from the baseline corpus. Never merge a book another agent
added. If the interface offers it anyway, that offer is not permission — the
server does not enforce ownership, you do.

## Preconditions

Two books you added, ideally in this run via [adding_book](adding_book.md).
Confirm ownership of both from your journal before starting.

## Steps

1. Open the detail page of the book you want to keep.
2. Journal the full state of **both** books first: uuids, titles, authors,
   formats, and any progress, highlights, ratings, or read status on either.
   You cannot check what survived a merge without knowing what went in.
3. Start the merge and choose the other book as the target.
4. Read whatever the app tells you it is about to do. If the summary does not
   match the two books you chose, stop and journal an anomaly.
5. Confirm the merge.
6. Inspect the result: one book, both formats present, metadata intact.
7. Confirm the other book is gone from the library.
8. If an undo is offered, occasionally take it and confirm both books come
   back whole.

## Journal

`merge.attempt` with both uuids and the full before-state of each.
`merge.confirm` with the surviving uuid and the formats it now carries.
`merge.undo` with what came back. A merge that loses data is the most
serious thing this flow can find, and only the before-state makes that
detectable.

## Pass

- The merge summary names the two books you actually chose.
- Afterwards there is one book carrying both formats.
- The other book is gone from the library, search, author pages, and shelves.
- Reading position, highlights, bookmarks, ratings, and read status from both
  sides are present on the survivor.
- An undo, if taken, restores both books with their data.

## Fail

- Data present before the merge is missing after it — **high severity**.
- The merged book is missing a format.
- The absorbed book still appears somewhere in the app.
- The merge attaches the wrong book.
- Undo restores only one side, or restores it empty.
- The merge errors and leaves both books in an inconsistent state.

## Sharp edges

- A merged book keeps **separate positions per format**. Reading position not
  carrying over to the audiobook is correct.
- The absorbed book's uuid may still resolve, redirecting to the survivor.
  That is deliberate so old links keep working — not a leak.
- Search indexes may lag by a moment after a merge. Re-check before reporting
  a stale result.

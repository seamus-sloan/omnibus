# Merging two books

| | |
|---|---|
| **Runs** | inside adding_book |
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
Confirm ownership of both from your journal before starting. The runner checks
this before handing the flow over and withholds it from an agent that owns
fewer than two; if you were handed it anyway and own only one, journal that
and end `uncertain` — do not upload a second book to make the numbers work.

The detail page's **Link Formats** control offers the same join from the other
direction when two books are one work in two formats. It is a merge under
another name, and the same ownership rule applies to both books.

## Steps

1. Open the detail page of the book you want to keep.
2. Journal the full state of **both** books first: uuids, titles, authors,
   formats, and any progress, highlights, ratings, or read status on either.
   You cannot check what survived a merge without knowing what went in.
3. Start the merge and choose the other book. Read which one the dialog says
   is **kept** — the app's model is that everything folds into the entry you
   are viewing — and journal it, so a renamed survivor can be told from a
   merge that went the wrong way.
4. Read whatever the app tells you it is about to do. If the summary does not
   match the two books you chose, stop and journal an anomaly.
5. Confirm the merge.
6. Inspect the result: one book, both formats present, metadata intact.
7. Confirm the other book is gone from the library.
8. If an undo is offered, take it and confirm both books come back whole.
   **The undo is a toast on the page you merged from, and it does not survive
   a reload** — so take it before the one reload pitfalls.md allows, and check
   every field of your before-state on both books afterwards: title, authors,
   series and number, tags, genres, description, ISBN, rating, read status,
   position, highlights, journal entries. Check whose journal entries sit on
   each book; an entry that belongs to another reader landing on the survivor
   and staying there after the undo is high severity.

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
- Highlights and bookmarks from both sides are present on the survivor — these
  accumulate, so "both" is literal.
- For the **scalar** values that cannot merge — rating, read status, reading
  position — the survivor keeps a coherent single value, and you journal which
  side it came from. There is no documented conflict rule, so either side
  winning is acceptable; what is not acceptable is a value belonging to neither,
  or one side's value landing on the wrong book.
- An undo, if taken, restores both books with **their own** data — each book
  carrying the rating, read status and identifiers it had before the merge, not
  the other's. Check both books, not just the survivor.

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
- A merged ebook-plus-audiobook book gains the **immersive read** invitation
  on its detail page. Seeing it appear after the merge is correct; if you
  drew [deleting_a_book](deleting_a_book.md) too, deleting one format should
  take it away again.
- **The audit does not verify merges.** `merge.*` is out of its scope because
  the result is library-wide. Your before-state journal entries are the only
  record that can show what a merge lost.

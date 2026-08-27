# Adding a book

| | |
|---|---|
| **Weight** | 10% |
| **Owner-only** | n/a — **this flow is what creates ownership** |
| **Surfaces** | web |
| **Actions** | `book.add`, `book.add.confirm` |

Upload a book from the corpus. This is the only flow that grows the library,
and its `book.add` journal entry is what makes you the owner of the result —
the entry that later authorises you, and only you, to merge or delete it.

**If a `book.add` entry is missing or lacks the resulting uuid, ownership of
that book is lost forever.** Journal it before you do anything else with the
book.

## Preconditions

The corpus path handed to you at spawn. Pick a file you have not added before;
the harness tells you which of the corpus you have already used.

**The corpus is something you upload, not something you install.** Every book
in this flow reaches the library by going through the app's Add-books screen.
Copying a file straight into the library directory is never a shortcut for
this flow — it is the opposite of it, since the upload path is precisely what
is under test, and a book that arrives that way has no owner. See *The corpus
goes in through the front door* in [start.md](../start.md).

## Steps

1. Click **Add books** in the nav. Do not type a path — there is no `/add`, and
   guessing one lands you on a page that is not part of the app.
2. Choose the **Upload type**: *Ebook* or *Audiobook*. It matters — an ebook
   takes a single file, an audiobook takes several at once.
3. Choose a file from the corpus, by dropping it on the drop zone or through
   the file chooser. Journal the filename **before** you upload it.
4. The app extracts the file's metadata and shows a **review form** — Title,
   Author, Series, Series index — under "Review the details, then add to your
   library." Read it against what you know the book to be. Real library files
   frequently carry garbled, swapped, or filename-derived metadata, and this
   form is where a person would fix it before committing. Correct it when it is
   wrong, and journal both what the app offered and what you changed.
5. Click **Add to library**.
6. Wait for the book to appear in the library. Indexing is asynchronous — give
   it time and re-check rather than reporting it missing straight away.
7. Open its detail page from the library grid. Confirm the cover, title, author,
   format, and page or chapter count are plausible for that book.
8. **Journal `book.add` with the resulting uuid.** This is the ownership record.
9. Half the time, add a **second** book and then run
   [merging_books](merging_books.md) against the pair.

Two refusals you may legitimately meet, both correct behaviour: "Title and
author are required." if you clear those fields, and "You don't have permission
to add books to this library." if your account lacks upload rights. Journal
either as `refused`, not as a failure.

## Journal

`book.add` carrying the source filename, the resulting **uuid**, the detected
format, and the extracted title and author. If the upload failed, journal it
with `outcome: error` and the message — a rejected upload is as interesting as
an accepted one.

## Pass

- The upload is accepted and reports progress or completion.
- The book appears in the library within a reasonable wait.
- Title, author, and cover were extracted from the file and are plausible.
- The detail page opens and the book can be read or played.
- Adding a second, different file produces a second, distinct book.

## Fail

- The upload reports success but no book appears, after a generous wait.
- The book appears with no title, no author, or a cover from a different book.
- The upload errors on a file that is a valid book.
- Uploading a book silently replaces or merges into an existing one you did not
  intend.
- The book appears but cannot be opened.

## Sharp edges

- **A book whose title and author match an existing one may attach to it as a
  second format rather than becoming a new book.** That is a deliberate
  feature, not a defect — but journal it clearly when it happens, because it
  changes what you own.
- Large audiobook files take a while. Judge by whether progress is being made,
  not by elapsed time alone.
- A file the app legitimately does not support should be refused with a clear
  message. A clear refusal is a pass; a silent one is a fail.

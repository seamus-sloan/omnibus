# Adding a book

| | |
|---|---|
| **Runs** | on its own |
| **Owner-only** | n/a — **this flow is what creates ownership** |
| **Surfaces** | web |
| **Actions** | `book.add`, `book.add.verify` |

Upload a book from the corpus. This is the only flow that grows the library,
and its `book.add` journal entry is what makes you the owner of the result —
the entry that later authorises you, and only you, to merge or delete it.

**If a `book.add` entry is missing or lacks the resulting uuid, ownership of
that book is lost forever.** Journal it before you do anything else with the
book.

## If the runner briefed you as a reader

Read this before the preconditions, because they do not apply to you. A reader
whose upload permission has been turned off runs this flow to meet the
**refusal**, and that is the whole flow:

- The **pass** is that every route to an upload ends at "You don't have
  permission to add books to this library." — no file input, no drop zone, no
  upload-type selector, no Add-to-library button.
- The **fail** is a screen that lets you upload anyway.
- **A hidden entry point is not a failure to reach the refusal.** The desktop
  nav has no Add books item for you at all; the phone-width tab bar still
  offers one, and its sheet still advertises "Upload a file", which is #2526.
  Reaching the refusal by either route is a pass; say which routes exist.
- Skip the corpus requirement and the file-chooser caveat below — you upload
  nothing, so neither applies, and stopping at them would report `uncertain`
  for a flow that reached a clean refusal.
- Journal the attempt `refused`, with `source_filename` and the target left
  null. The journal contract below assumes an upload happened; a refusal has
  no filename and no uuid.

## Preconditions

**You need a browser tool that can actually put a file into a file input.** The
upload is a real `<input type="file">`, and the page's own CSP (`connect-src
'self'`) blocks every in-page workaround, so an agent whose browser cannot drive
a file chooser cannot perform this flow. If yours cannot, stop and journal it
`uncertain` — say so plainly and move on.

**Never fall back to `curl` or a direct API call to get the book in.** That
uploads the file while skipping the entire client-side path this flow exists to
exercise, and it would report a pass for a broken uploader. An upload you did
not perform through the screen is not this flow.

The corpus path handed to you at spawn, **and the list of corpus files already
uploaded** — the runner derives it from every run's journals and hands it to
you in the brief, and when two agents draw this flow in one run the runner
also names which file is yours. Pick a file on neither list. If you were
handed no such list, ask before uploading rather than guessing: a file added
twice attaches to the first copy as a second format and changes what you own.

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
   the file chooser. Journal the filename **before** you upload it, as
   `book.add` with `outcome: uncertain` and no target — the audit skips a
   non-`ok` entry, and the `ok` one at step 8 supersedes it.
4. The app extracts the file's metadata and shows a **review form** under
   "Review the details, then add to your library." Read it against what you know
   the book to be. Real library files frequently carry garbled, swapped, or
   filename-derived metadata, and this form is where a person would fix it
   before committing. Correct it when it is wrong, and journal both what the app
   offered and what you changed.

   Three things to expect rather than report as bugs. The form may come back
   **entirely empty** — audiobooks often carry no container tags at all, and
   there is then nothing to review; fill in Title and Author yourself, since the
   required-field guard will otherwise block the save — with the book's *real*
   title and author, read off the filename or folder, never a placeholder like
   `Untitled` or `Test Audiobook`. The book stays in the shared library after
   you leave. Both forms offer **Series** fields; the audiobook parser rarely
   fills them, so this is the only place a series can be supplied. And when
   the file names several creators, the form shows the first in Author and
   lists the rest beneath it — they are imported as additional creators, and
   editing Author replaces only the first. Journal every name the form showed,
   then confirm them on the detail page.
5. Click **Add to library**.
6. The app lands on the new book's detail page itself once the add finishes.
   Go back to the library and confirm the book is there too; indexing is
   asynchronous — give it a moment and re-check rather than reporting it
   missing straight away. The library may be in table view from an earlier
   flow.
7. Open its detail page from the library. Confirm the cover, title, author,
   format and identifiers are plausible for that book. The detail page shows
   no page count, no chapter count, and no publication-date row — do not go
   looking for them. A first open in the reader that fails with "This book
   couldn't be loaded" and then works after a reload is a finding in its own
   right; journal it.
8. **Journal `book.add` with the resulting uuid.** This is the ownership record.
9. Once the book can be opened, journal `book.add.verify` with what the detail
   page showed. The trailing `.verify` is how you say "I checked it stuck";
   do not invent another name for it.

Two refusals you may legitimately meet, both correct behaviour: "Title and
author are required." if you clear those fields, and "You don't have permission
to add books to this library." if your account lacks upload rights — which is
the case when the runner briefed you as a **reader**; then the refusal is the
whole flow, and a screen that lets a reader upload anyway is the finding.
Journal either as `refused`, not as a failure.

## Journal

`book.add` carrying `source_filename` (the file's name as it is in the corpus
— the runner reads this key to build the used-files list), the resulting
**uuid**, the detected format, and the extracted title and author. If the
upload failed, journal it with `outcome: error` and the message — a rejected
upload is as interesting as an accepted one.

## Pass

- The upload is accepted and reports progress or completion.
- The book appears in the library within a reasonable wait.
- Title, author, and cover were extracted from the file and are plausible.
- The detail page opens and the book can be read or played.
- Adding a second, different file produces a second, distinct book — only
  when the runner handed you two files.

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
  not by elapsed time alone. A large file refused with a server error after
  about thirty seconds is a finding, not a slow upload — journal the size and
  the timing.
- A cover declared only through the EPUB2 manifest and guide, with no
  `<meta name="cover">`, may not be extracted even though the reader renders
  it. Journal it as a finding with the file named.
- A file the app legitimately does not support should be refused with a clear
  message. A clear refusal is a pass; a silent one is a fail.

## Correction from run r-20260908-02

Step 7 previously said the detail page shows no page count, no chapter count
and no publication-date row. It does show `Published`, and a chapter count on
the progress line once the reader has been opened; only the page count is
absent.

## When the file names an extra creator

The review form lists any creator beyond the first beneath the Author field,
and they are not always people. A great many EPUBs carry a Calibre producer
stamp — `calibre (3.48.0) [https://calibre-ebook.com]` — as a `dc:contributor`,
and it is imported as an author: it appears in the grid author line, the
detail-page byline and the Authors index, and the review form offers no way to
drop it. **That is a finding, not an expected artifact** (#2501). Journal the
extra creator verbatim and say whether it looks like a person or a toolchain.

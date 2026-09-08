# Browsing a book's details

| | |
|---|---|
| **Runs** | on its own |
| **Owner-only** | no |
| **Surfaces** | web, iOS |
| **Actions** | `book.view`, `rating.set`, `status.set`, `nav.follow`, `ui.scroll_stops` |

Open a book and look at it without reading it — the way you do when deciding
whether to. This flow is the widest surface in the app and the most likely to
turn up something cosmetic that no assertion would catch.

## Steps

1. Reach a book's detail page from wherever you happen to be.

   **In the library's table view, click the book's cover.** The cover cell is
   the row's one navigation affordance — it is the link named "Open details
   for …", and it is what the keyboard reaches. Most of the other cells
   (title, author, series, tags, genres, published, language) are inline
   editors that open themselves on a click. Clicking a title and landing in an
   edit box is the table working as designed; it is **not** a broken row, and
   must not be journalled as one. In grid view, click the tile.
2. Actually read the page. Cover, title, author, series, description, formats,
   dates, identifiers, tags, genres, ratings, saved passages, suggestions.
   There is **no page count** on this page, and a chapter figure appears only
   once a position exists ("Ch. 7 of 13" in the HOME section) — do not report
   its absence on a book you have not opened.
3. Ask of each thing: is this plausible? A publication year of 0101, a date in 1900,
   an author of "Unknown, Unknown", a description that is raw HTML, a cover
   that belongs to another book — all findings.
4. Then do **each** of the following, in whatever order you like. You do not
   choose between them — a flow never samples its own steps:
   - **Rate it.** Set a rating, confirm it takes, reload and confirm it stuck.
     Then clear it, confirm the clear sticks, and set it once more — leave a
     rating behind, because the audit reconciles against one.
   - **Set a read status.** The three states are **Unread**, **Reading** and
     **Finished** — there is no "want" here; the wishlist is a separate thing
     reached through Check in. Move between them and watch the home page.
   - **Follow every link the page offers**, and confirm each destination is
     about the thing you clicked. The page can carry all of these; say which
     it had, and journal `nav.follow` for each one you took:
     - the **series** name — the one link reliably present on a series book;
     - the **author** — the lead name, and the "+N more" beside it on a book
       with several creators, which goes to the author page too. Some books
       show their author as plain text with no link; that *is* worth
       journalling, as an observation about that book rather than a failure;
     - the **file picker**, on a book with more than one file of a format —
       each entry opens the reader or player on that file;
     - **"Open in reader"** on a saved passage in the highlights list — it must
       open the reader *at that passage*, not at the start;
     - a **suggestion's** outward link, which leaves the app for Hardcover;
       journal that it opened the right book there and come back;
     - the physical pill's **find** link on a book with a paper copy;
     - **Link Formats**, on a book with one format whose other format exists
       as a separate book — it offers to join them. Read what it offers and
       journal it; take it only if **you own both** books, the same rule as
       [merging_books.md](merging_books.md).
     Tag and genre chips are not links and go nowhere — that is correct. The
     export menu's downloads are for you to *see*, not to take: a download
     fetches a real file and Send to Kindle is on the rails. A book with
     **none** of the optional links — one author, one file, no passages, no
     paper copy, no other format, suggestions off — is a common and legitimate
     draw; journal which were absent rather than hunting.
   - **Look at the suggestions.** These come from Hardcover and need a
     server-wide API key. If the panel offers an "Add a Hardcover API key" CTA
     instead of books, the feature is switched off — that is **not** a finding,
     and you must not go to Settings to enable it. Note it and follow the
     sibling "More by <author>" panel instead.
   - **If the book has both an ebook and an audiobook**, look for the
     immersive-read invitation and the two separate positions. Reading
     position and listening position are independent; both showing is
     correct, and one moving because the other did is not.
   - **Flip the scroll-stop layout.** The detail page has two shapes, chosen
     by a toggle on **your own** account page (Settings → Account on the web,
     reached from the user menu's Edit link; You → Book details on iOS): off
     (the default) is one
     continuous page with the sections introduced by rules; on snaps the page
     section by section with a dot rail. Go to your account page, turn it on,
     come back to this book, and confirm the page now snaps and the dot rail
     is there. Read the same sections in the new layout — nothing should be
     missing that the other layout had. Then turn it off again and confirm the
     continuous page is back. Journal `ui.scroll_stops` with each change and
     what you saw. Judge this on what the page *contains*, not on how the
     snapping feels: whether every section is reachable, whether the dot rail
     moves you between them, whether anything is clipped or doubled.
5. Come back to the book and confirm the page is as you left it.

## Journal

`book.view` with the uuid and a short note of anything that looked off, even if
you are not sure. `rating.set` and `status.set` with old and new values —
these are per-user state the audit will check. `nav.follow` with what you
clicked and where you landed, one per link. `ui.scroll_stops` with the value
you set and which layout the page then showed; it is account configuration
and the audit does not check it.

## Pass

- The page renders completely, with no missing sections or placeholder text.
- Every field is plausible for the book it describes.
- Rating and read status take effect immediately and survive a reload.
- Links go where their label says — every one you found, not just the first.
- Both detail layouts show the same sections, and the toggle between them
  sticks across a reload.

## Fail

- Fields belonging to a different book.
- A rating or status that reverts, or lands on the wrong book.
- A link to an author or series that 404s, or shows an unrelated one.
- A section that spins forever or shows an error.
- Someone else's rating shown as yours.
- "Open in reader" on a passage opens the book at the start, or a different
  passage.
- A section present in one layout and missing, clipped or duplicated in the
  other; or a toggle that does not change the layout after a reload.

## Sharp edges

- Not every book has a series, a description, or suggestions. Absent is fine;
  broken is not. Say which you saw.
- Suggestions come from an external service and may legitimately be empty or
  slow. An empty suggestions panel is not a finding; an error in it is.
- **Another agent may have edited this book's metadata a moment ago.** Fields
  changing between two visits is expected in a shared library.
- Read status changes move a book **off** the continue surface (Finished
  removes it) but do not necessarily put one **on**: marking a never-opened book
  as Reading adds nothing, because the surface is driven by reading progress.
  Both directions are correct.
- The scroll-stop toggle is yours alone. Another agent seeing the other
  layout on the same book is two settings, not one bug. Leave it **off** when
  you are done, so the next flow you draw sees the default.
- On iOS the series name is plain text, not a link, and there is no control
  that clears a rating (set and re-set work). Journal both as observations
  about the surface, not failures of the step.
- Setting a never-opened book to **Reading** has been observed to mint a 0%
  position and put the book on the continue fan. Whether that is intended is
  open; journal exactly what the home page and the detail page show before
  and after the status change rather than deciding.

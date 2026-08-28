# Browsing a book's details

| | |
|---|---|
| **Weight** | 20% |
| **Owner-only** | no |
| **Surfaces** | web, iOS |
| **Actions** | `book.view`, `rating.set`, `status.set`, `nav.follow` |

Open a book and look at it without reading it — the way you do when deciding
whether to. This flow is the widest surface in the app and the most likely to
turn up something cosmetic that no assertion would catch.

## Steps

1. Reach a book's detail page from wherever you happen to be.
2. Actually read the page. Cover, title, author, series, description, formats,
   dates, identifiers, tags, genres, ratings, saved passages, suggestions.
   There is **no page or chapter count** on this page — do not go looking for
   one, and do not report its absence.
3. Ask of each thing: is this plausible? A negative page count, a date in 1900,
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
   - **Follow a link out** and confirm the destination is about the thing you
     clicked. **Series is the only one reliably present.** Tag and genre chips
     on a book page are inert, and some books show their author as plain text
     with no link. A missing author link *is* worth journalling — but as an
     observation about that book, not as a failure of this step.
   - **Look at the suggestions.** These come from Hardcover and need a
     server-wide API key. If the panel offers an "Add a Hardcover API key" CTA
     instead of books, the feature is switched off — that is **not** a finding,
     and you must not go to Settings to enable it. Note it and follow the
     sibling "More by <author>" panel instead.
5. Come back to the book and confirm the page is as you left it.

## Journal

`book.view` with the uuid and a short note of anything that looked off, even if
you are not sure. `rating.set` and `status.set` with old and new values —
these are per-user state the audit will check. `nav.follow` with what you
clicked and where you landed.

## Pass

- The page renders completely, with no missing sections or placeholder text.
- Every field is plausible for the book it describes.
- Rating and read status take effect immediately and survive a reload.
- Links go where their label says.

## Fail

- Fields belonging to a different book.
- A rating or status that reverts, or lands on the wrong book.
- A link to an author or series that 404s, or shows an unrelated one.
- A section that spins forever or shows an error.
- Someone else's rating shown as yours.

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

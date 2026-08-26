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
   page count, dates, tags, genres, ratings, saved passages, suggestions.
3. Ask of each thing: is this plausible? A negative page count, a date in 1900,
   an author of "Unknown, Unknown", a description that is raw HTML, a cover
   that belongs to another book — all findings.
4. Then pick **one** of the following, at roughly equal odds:
   - **Rate it.** Set a rating, confirm it takes, come back later and confirm it
     stuck. Occasionally clear it.
   - **Set a read status.** Move it between want, reading, and finished, and
     watch what the home page does in response.
   - **Follow a link out** — to the author, the series, a tag, or a genre — and
     confirm the destination is about the thing you clicked.
   - **Look at the suggestions** and follow one.
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
- Read status changes here **do** move the book on and off the home page's
  continue surface. That is the feature.

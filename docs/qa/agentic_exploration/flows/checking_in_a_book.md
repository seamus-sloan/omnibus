# Checking in a physical book

| | |
|---|---|
| **Runs** | on its own |
| **Owner-only** | filing a copy: no; removing one: **yes** — see below |
| **Surfaces** | web, iOS |
| **Actions** | `checkin.start`, `checkin.lookup`, `checkin.confirm`, `book.add`, `checkin.remove` |

Record a print copy of a book. The same **Check in** screen that
[wishlist](wishlist.md) uses has two other outcomes, and this flow is about
those: filing a copy against a book you already have digitally, and adding a
book you own only on paper.

**Physical copies are library-wide.** A PHYS badge in the library, a physical
pill on the detail page, and the ISBN binding the copy files under are seen by
every reader and outlive the run. The ownership guard treats copy removal as
destructive, so **you can only remove a copy from a book you own** — file
copies against books you uploaded, or against a paper-only book you create
here, never against the baseline corpus.

## Steps

1. Click **Check in** in the nav. On the web it opens a dialog over the page
   you are on, not a page of its own; on iOS it is behind the Library
   masthead's `+` → Add books → Scan a barcode, which on a simulator offers
   ISBN and title fields instead of the camera.
2. Enter an ISBN, or search by title. **There is no author field** — a title
   alone, as [wishlist.md](wishlist.md) says.
3. Read the candidates. The lookup checks your library first, then external
   services, and the outcome it offers depends on what it found:
   - **"In your physical collection"** — you already filed this one. Correct;
     journal it and pick another book.
   - **"Check in this copy"** (iOS: **"I already have this book"**) — the book
     exists digitally and this adds the print copy. An **"Is this the book?"**
     interstitial comes first, because print and digital editions carry
     different ISBNs; the app asks before filing against the wrong book. Take
     this path for a book **you uploaded**. An edition note can be written
     here as well as later.
   - **"I own it"** (iOS: **"Add as physical book"**) with no digital match —
     creates a paper-only book and its first copy. Take this path for a real
     book that is not in the library.
     **Journal `book.add` for the result with its uuid**: a book you created
     here is one you own, exactly as an upload would be, and without that
     entry nobody can ever remove it.
   - **On your wishlist** — the book is a wishlist entry; checking a copy in
     should turn it into a real entry. Only try this on a book *you*
     wishlisted earlier.
4. Confirm. Watch for the confirmation naming the right book.
5. Open the book's detail page and confirm the physical copy is shown — on
   the web a **Physical copy** card under THE FILES with an "In your physical
   collection" marker; on iOS a Physical copy row and an "On your shelf —
   physical copy" bar. Then confirm the library shows it: the web **table**
   view's Formats cell carries PHYS; grid tiles carry no badge on either
   surface.
6. If the copy card offers a note — where it lives, its condition — write
   one a person would write, journal it as `checkin.note`, and confirm it
   sticks after a reload. iOS offers no note.
7. Re-run the same lookup. The web navigates straight to the book with no
   message; iOS shows an "Already on your shelf" card. Both are recognition.
8. Occasionally, on a book **you own**, remove the copy — the web's control
   is labelled "I sold it" — and confirm the card and badge go. Removing the
   **last** copy of a paper-only book removes the book, after a dialog that
   offers to move it to the wishlist instead; that is correct. iOS has no
   remove control, so the book stays; say so. On a book you do not own the guard will refuse with a `403`
   carrying `ownership_guard`; journal that `refused` and do not look for
   another route.

## Journal

`checkin.lookup` with the query and the candidates, as in
[wishlist.md](wishlist.md). `checkin.confirm` with the path taken, the book's
uuid, and the ISBN filed. `checkin.note` with the note text. `book.add` for
a paper-only book, with the uuid and the title and author the app recorded
(on iOS, the title in `params.title`). `checkin.remove` with the uuid.

**The audit does not verify copies.** Physical copies are library-wide state,
and the vocabulary lists `checkin` as out of scope; the `book.add` for a
paper-only book is the one entry here it checks — and a `checkin.remove` on
that book supersedes it, so a paper-only book you created and then removed
is expected to be gone. Your journal is still the
record — a copy that appears with nothing journalling it is what the runner
will ask about.

## Pass

- The lookup offers the outcome that matches the book's real state in the
  library.
- Filing confirms, and the pill and badge appear.
- A paper-only book appears in the library with the title and author the
  lookup returned, and its detail page opens.
- The same lookup afterwards recognises the copy.
- Removal on an owned book removes the copy and nothing else.

## Fail

- A copy files against a different book than the one confirmed.
- A paper-only book appears with empty or wrong metadata.
- The pill or badge is missing after a reload.
- Removing a copy from a book that **has files** deletes the book, or
  removes a different copy. High severity. (Removing the last copy of a
  paper-only book removes the book by design.)
- The lookup offers "I own it" for a book that is plainly in the library.

## Sharp edges

- The external lookup is a third-party service: it can be slow, empty, or
  rate-limited. The app must say so clearly; a clear message is `uncertain`,
  a hang is a fail.
- A paper-only book has no file, so it cannot be read or played. Its detail
  page saying so is correct.
- Another agent may file a copy against the same book while you look at it.
  Two copies of one book is a legitimate state.

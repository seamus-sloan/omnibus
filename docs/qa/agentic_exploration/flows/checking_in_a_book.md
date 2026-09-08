# Checking in a physical book

| | |
|---|---|
| **Weight** | 3% |
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

1. Click **Check in** in the nav.
2. Enter an ISBN, or search by title. **There is no author field** — a title
   alone, as [wishlist.md](wishlist.md) says.
3. Read the candidates. The lookup checks your library first, then external
   services, and the outcome it offers depends on what it found:
   - **"In your physical collection"** — you already filed this one. Correct;
     journal it and pick another book.
   - **"Check in this copy"** — the book exists digitally and this adds the
     print copy. Read the note about print and digital editions carrying
     different ISBNs; the app asks before filing against the wrong book.
     Take this path for a book **you uploaded**.
   - **"I own it"** with no digital match — creates a paper-only book and its
     first copy. Take this path for a real book that is not in the library.
     **Journal `book.add` for the result with its uuid**: a book you created
     here is one you own, exactly as an upload would be, and without that
     entry nobody can ever remove it.
   - **On your wishlist** — the book is a wishlist entry; checking a copy in
     should turn it into a real entry. Only try this on a book *you*
     wishlisted earlier.
4. Confirm. Watch for the confirmation naming the right book.
5. Open the book's detail page and confirm the physical pill is there, and
   that the library grid shows the PHYS badge on it.
6. If the pill offers a note on the copy — where it lives, its condition —
   write one a person would write, and confirm it sticks after a reload.
7. Re-run the same lookup and confirm the app now says you have it.
8. Occasionally, on a book **you own**, remove the copy and confirm the pill
   and badge go. On a book you do not own the guard will refuse with a `403`
   carrying `ownership_guard`; journal that `refused` and do not look for
   another route.

## Journal

`checkin.lookup` with the query and the candidates, as in
[wishlist.md](wishlist.md). `checkin.confirm` with the path taken, the book's
uuid, and the ISBN filed. `book.add` for a paper-only book, with the uuid and
the title and author the app recorded. `checkin.remove` with the uuid.

**The audit does not verify copies.** Physical copies are library-wide state,
and the vocabulary lists `checkin` as out of scope; the `book.add` for a
paper-only book is the one entry here it checks. Your journal is still the
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
- Removing a copy deletes the book, or removes a different copy. High
  severity.
- The lookup offers "I own it" for a book that is plainly in the library.

## Sharp edges

- The external lookup is a third-party service: it can be slow, empty, or
  rate-limited. The app must say so clearly; a clear message is `uncertain`,
  a hang is a fail.
- A paper-only book has no file, so it cannot be read or played. Its detail
  page saying so is correct.
- Another agent may file a copy against the same book while you look at it.
  Two copies of one book is a legitimate state.

# Adding a book to your wishlist

| | |
|---|---|
| **Weight** | 5% |
| **Owner-only** | no — a wishlist is per-user |
| **Surfaces** | web, iOS |
| **Actions** | `checkin.start`, `checkin.lookup`, `wishlist.add`, `wishlist.remove` |

A wishlist entry is a book you do not have. It is reached through **checking in
a book**, not through a button on a library page — the app looks the book up on
the web first, and the wishlist is one of the outcomes it offers.

That lookup is the interesting part. It goes out to real external services, so
this flow exercises a code path that depends on the network and can legitimately
fail.

## Steps

1. Start checking in a book.
2. Identify a book you do **not** already have. Either enter an ISBN or search
   by title and author. Prefer a real, well-known book — the lookup has a better
   chance and a wrong answer is easier to spot.
3. Read what comes back. Does the result match what you asked for? Journal the
   candidates you were shown.
4. Pick the right one, or say none of them match if none do.
5. When offered what to do with it, choose to add it to your wishlist.
6. Confirm it lands there, and that revisiting the same book afterwards shows
   it as already on your wishlist.
7. Occasionally remove it again and confirm it goes.

## Journal

`checkin.lookup` with what you searched for and the candidates returned —
titles and authors, in order. `wishlist.add` with the chosen title, author, and
ISBN, plus the resulting entry's identifier if one is shown.

## Pass

- The lookup returns plausible candidates for what you asked.
- Choosing one shows a confirmation naming that book.
- The entry appears on your wishlist with the right title, author, and cover.
- Revisiting the same book reports it as already on the wishlist.
- Removal works and the entry goes.

## Fail

- The lookup returns a book unrelated to the query.
- A book already in the library is offered as a wishlist candidate rather than
  being recognised as owned.
- The entry lands with the wrong metadata, or with none.
- The wishlist shows another user's entries. **High severity** — wishlists are
  per-user.
- Removing an entry removes a different one.

## Sharp edges

- **A failed lookup is not automatically a bug.** These are third-party
  services and they go down, rate-limit, and return nothing. What matters is
  that the app says so clearly rather than hanging or showing an empty screen.
  Journal it as `uncertain` with the message you saw.
- The lookup checks your library first. Being told you already own a book you
  do own is correct.
- Adding to the wishlist requires a connection — it needs the server's lookup
  before it has anything to save. **The iOS agent must not attempt this while
  offline.**

# Adding a book to your wishlist

| | |
|---|---|
| **Runs** | on its own |
| **Owner-only** | no — a wishlist is per-user |
| **Surfaces** | web, iOS |
| **Actions** | `checkin.start`, `checkin.lookup`, `wishlist.add`, `wishlist.remove` |

A wishlist entry is a book you do not have. It is reached through **checking in
a book**, not through a button on a library page — the app looks the book up on
the web first, and the wishlist is one of the outcomes it offers. The other
outcomes — filing a print copy — are
[checking_in_a_book](checking_in_a_book.md); this flow takes the wishlist
path only.

That lookup is the interesting part. It goes out to real external services, so
this flow exercises a code path that depends on the network and can legitimately
fail.

## Steps

1. Start checking in a book.
2. Identify a book you do **not** already have. Either enter an ISBN or search
   by title. **There is no author field** — the search takes a title only, and
   adding the author to it measurably degrades the results, so do not. Prefer a
   real, well-known book: the lookup has a better chance and a wrong answer is
   easier to spot.
3. Read what comes back. Does the result match what you asked for? Journal the
   candidates you were shown.
4. Pick the right one, or say none of them match if none do.
5. When offered what to do with it, choose to add it to your wishlist.
6. Confirm it lands there — on your wishlist shelf in the rail, and on the
   book's own page, whose header reads ON YOUR WISHLIST. Then revisit the same
   book **from a different page**: on the web, picking the candidate again
   navigates to that entry page with no message in the dialog, and the
   candidate list carries no marker; on iOS the confirmation screen says ON
   YOUR WISHLIST and withdraws the Add option. Either is recognition; a second
   entry is the fail.
7. Occasionally remove it again and confirm it goes. Removal has no
   confirmation and no undo on the web.

## Journal

`checkin.lookup` with what you searched for and the candidates returned —
titles and authors, in order. `wishlist.add` with the chosen title and author;
the ISBN is not shown at add time and appears only on the entry's own page
afterwards, so add it to a later `wishlist.add.verify`. Put the book's uuid in
`target` — read it from the shelf tile or the entry page's address, since the
confirmation card shows none — because that is what the audit looks for on
your wishlist shelf; without it the entry is journalled but not checked.
`wishlist.remove` with the same.

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
- **Your own** wishlist shelf shows another user's entries. **High severity**
  — wishlists are per-user. Every reader's wishlist *shelf* is listed for
  everyone, marked Public; that is by design and not this criterion.
- Removing an entry removes a different one.

## Sharp edges

- **A failed lookup is not automatically a bug.** These are third-party
  services and they go down, rate-limit, and return nothing. What matters is
  that the app says so clearly rather than hanging or showing an empty screen.
  Journal it as `uncertain` with the message you saw — this overrides
  start.md's "a 5xx is a fail" for the lookup alone, and the status code the
  outage is reported under is worth journalling as an observation.
- The lookup checks your library first, but only once you pick a candidate:
  the candidate list itself does not mark owned books. Being told you already
  own a book you do own is correct.
- A wishlist entry has no files, so its page offering "Delete files…" is
  worth journalling as a low observation, not a fail.
- Adding to the wishlist requires a connection — it needs the server's lookup
  before it has anything to save. **The iOS agent must not attempt this while
  offline.**

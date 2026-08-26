# Sorting the library and using shelves

| | |
|---|---|
| **Weight** | 12% |
| **Owner-only** | no; shelves are per-user |
| **Surfaces** | web, iOS |
| **Actions** | `library.sort`, `library.view`, `shelf.select`, `shelf.create`, `shelf.add`, `shelf.remove` |

Rearrange the library and put books on shelves. Sorting is pure presentation
and cheap to check; shelf membership is per-user state the audit verifies.

## Steps

1. Go to the library.
2. Change the sort order. Journal the first few titles before and after.
3. Verify the new order actually holds — read down the first several entries and
   confirm they are ordered the way the control claims.
4. Try a different view or density if one is offered.
5. Pick a shelf and confirm the library narrows to it, and that the count and
   the visible books agree.
6. Occasionally create a shelf of your own, then add two or three books to it
   and confirm they appear.
7. Occasionally remove a book from one of your shelves and confirm it goes from
   the shelf but **not** from the library.

## Journal

`library.sort` with the old and new sort, plus the first five titles under each.
`shelf.select` with the shelf name and resulting count. `shelf.create` with the
name and kind. `shelf.add` and `shelf.remove` with the shelf and the book uuids
— these are what the audit reconciles.

## Pass

- The chosen sort is reflected in the actual order, not just the control.
- The order is stable across a reload.
- Selecting a shelf narrows the library to that shelf's books, with a matching
  count.
- A created shelf appears and accepts books.
- Removing a book from a shelf leaves the book in the library.

## Fail

- The order does not match the chosen sort.
- Sorting drops books, duplicates them, or changes the total count.
- A shelf shows books that do not belong to it, or omits ones that do.
- Removing a book from a shelf deletes the book. **High severity.**
- Another user's shelves are visible to you.

## Sharp edges

- **On the web, choosing a shelf filters the library in place** and the address
  does not change. That is correct. Only the iOS agent gets a dedicated shelf
  screen with its own controls.
- The default sort is by recent interaction, so it moves as agents use the
  library. Two agents seeing different orders under that sort is expected.
- Books added by other agents appearing mid-flow will change counts underneath
  you. Re-read the count rather than trusting one you noted a minute ago.
- Some shelves fill by rule rather than by hand. Being unable to add a book to
  one of those directly is correct.

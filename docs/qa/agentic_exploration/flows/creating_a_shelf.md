# Creating a shelf

| | |
|---|---|
| **Weight** | 4% |
| **Owner-only** | no; shelves are per-user |
| **Surfaces** | web (create only), iOS (create and fill) |
| **Actions** | `shelf.create`, `shelf.select`, `shelf.edit` |

Make a shelf and confirm it exists, is yours, and behaves when selected.

## ⚠️ Read this before you start

**On the web you can create a shelf but you cannot put books on it.** There is
no add-to-shelf control anywhere in the web UI — not on a grid tile, not in the
table's bulk bar (its "Edit" is metadata only), not on a book's detail page, not
in **Edit shelf** (visibility and Kobo sync only), and not in the command
palette (which searches content, not actions). Shelf *membership* lives only on
the iOS shelf screen.

So a hand-picked shelf created on the web can only ever be empty. That is the
current design, not a bug, and **you must not spend the flow hunting for a
control that does not exist.** If you are on web, create the shelf, confirm it
selects, and end the flow — do not go looking.

## Steps

1. Go to the library. The shelves rail sits above the book list, starting with
   **All Books**.
2. Click **＋ New shelf**.
3. Name it something clearly yours, including your actor id.
4. Choose its visibility — **Private** or **Public** — and note whether you are
   making a hand-picked shelf or a **Smart** one (a smart shelf fills itself
   from a rule; a hand-picked one does not).
5. Click **Create**.
6. Confirm it appears in the rail, with the name, visibility, and kind you
   chose.
7. Select it and confirm the library narrows to it. A brand-new hand-picked
   shelf will be empty, and its count should say so.
8. Select **All Books** again and confirm the full library comes back.
9. Reload and confirm the shelf is still there.
10. **On iOS only:** open the shelf's own screen, add two or three books, and
    confirm they appear. Then remove one and confirm it leaves the shelf but
    **not** the library.

## Journal

`shelf.create` with the name, visibility, and kind. `shelf.select` with the name
and the resulting count. On iOS, `shelf.add` / `shelf.remove` with the shelf and
the book uuids — those are what the audit reconciles.

## Pass

- The shelf is created with the name, visibility, and kind you chose.
- It appears in the rail and survives a reload.
- Selecting it narrows the library; the count matches what is shown.
- Selecting All Books restores the full library.
- On iOS, added books appear and a removed book leaves the shelf but stays in
  the library.

## Fail

- The shelf saves under a different name, visibility, or kind.
- It does not appear, or disappears after a reload.
- Selecting it shows books that do not belong to it.
- Another user's shelves are visible to you.
- **On iOS:** removing a book from a shelf deletes the book. High severity.

## Sharp edges

- **Choosing a shelf on the web filters in place** — the address does not
  change. Correct.
- A **Smart** shelf fills by rule, so being unable to add a book to it by hand
  is correct on every surface.
- Your display name is denormalised into the wishlist shelf's name, so that one
  may carry an older name than your profile does. Journal it `uncertain` rather
  than deciding.

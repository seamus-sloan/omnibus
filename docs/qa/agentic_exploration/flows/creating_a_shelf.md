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

**On iOS, membership is where the web's missing control went.** You are the
agent expected to fill a shelf, so here is exactly where it lives:

- Shelves are at **You → Shelves**, a grid of shelf cards — not a rail above
  the library, and not on the Library tab at all.
- **Create** with the `+` in the navigation bar of that Shelves screen.
- **Open** a shelf by tapping its card; it *pushes its own screen*. It does not
  filter the library, and there is no "All Books" to go back to — you leave a
  shelf with the back button.
- **Add** books with the `+` in that shelf screen's navigation bar, or with the
  **Add the first book** button an empty hand-picked shelf offers in its place.
- **Remove** a book by **long-pressing its tile** on the shelf screen and
  choosing **Remove from shelf**. There is no visible remove button; the
  context menu is the whole control, and it appears only on hand-picked
  shelves — a Smart shelf's tiles do not offer it, correctly.

## Steps

Steps 1–2 and 6–8 below describe **the web surface**. The iOS equivalent of
each is in the box above; take that path instead, and do not report the web
control as missing when you are on iOS.

1. **(web)** Go to the library. The shelves rail sits above the book list,
   starting with **All Books**. **(iOS)** Go to **You → Shelves**.
2. **(web)** Click **＋ New shelf**. **(iOS)** Tap the `+` in the Shelves
   navigation bar.
3. Name it the way a reader would name a shelf — *Weeknight Reading*, *Books
   Dad Lent Me*, *Finish Before Winter*. Not your actor id, not a date, not
   `test`. Journal the name you chose; that is what the audit matches on, so
   the two must agree exactly. Do not reuse a name you have already used.
4. Choose its visibility — **Private** or **Public** — and note whether you are
   making a hand-picked shelf or a **Smart** one (a smart shelf fills itself
   from a rule; a hand-picked one does not).

   **iOS only shows you back a *public* choice.** The shelf screen's meta line
   reads "N books · Manual · Public", and a private shelf simply omits that
   last part — there is no "Private" label anywhere. So on iOS you can confirm
   Public directly and Private only by the absence of the marker. If you want
   the visibility you chose to be checkable at all on that surface, choose
   **Public**; if you choose Private, journal the choice and record the
   confirmation as `uncertain` rather than claiming the app agreed with you.
5. Click **Create**.
6. **(web)** Confirm it appears in the rail, with the name, visibility, and kind
   you chose. **(iOS)** Confirm the card appears on the Shelves grid; open it
   and read the name and meta line on its own screen.
7. **(web only)** Select it and confirm the library narrows to it. A brand-new
   hand-picked shelf will be empty, and its count should say so. **This step
   does not exist on iOS** — tapping a shelf there pushes its own screen rather
   than filtering anything, so open it and confirm it is empty instead.
8. **(web only)** Select **All Books** again and confirm the full library comes
   back. **There is no All Books on iOS**; back out of the shelf screen instead
   and confirm the library is untouched.
9. Reload and confirm the shelf is still there. **(iOS)** Pull to refresh —
   both the Shelves grid and a shelf's own screen support it.
10. **On iOS only:** on the shelf's own screen, add two or three books with the
    `+` (or **Add the first book**), and confirm they appear. Then long-press
    one, choose **Remove from shelf**, and confirm it leaves the shelf but
    **not** the library.

## Journal

`shelf.create` with the name, visibility, and kind. `shelf.select` with the name
and the resulting count. On iOS, `shelf.add` / `shelf.remove` with the shelf and
the book uuids — those are what the audit reconciles.

## Pass

- The shelf is created with the name and kind you chose — and with the
  visibility you chose, **on web, and on iOS only when you chose Public**
  (see step 4; a private shelf is unlabelled there and the criterion is
  `uncertain`, not a fail).
- It appears — in the web rail, or on the iOS Shelves grid — and survives a
  reload.
- **(web only)** Selecting it narrows the library; the count matches what is
  shown.
- **(web only)** Selecting All Books restores the full library.
- **(iOS)** In place of those two: tapping the shelf pushes its own screen,
  and that screen lists exactly the shelf's books.
- On iOS, added books appear and a removed book leaves the shelf but stays in
  the library.

## Fail

- The shelf saves under a different name, visibility, or kind.
- It does not appear, or disappears after a reload.
- Selecting it shows books that do not belong to it.
- Another user's shelf is visible **and you are not an admin**. Admins see every
  shelf by design, and the exploration accounts are currently all admins — so on
  this instance the criterion is undecidable and you should journal `uncertain`
  rather than guess. Non-admin exploration accounts are planned; once one exists
  this becomes a real, decidable fail criterion and the `uncertain` escape stops
  applying. (Worth reporting separately: another user's private shelf
  renders in the rail with no owner attribution, while the wishlists beside it
  do show owner names.)
- **On iOS:** removing a book from a shelf deletes the book. High severity.

## Sharp edges

- **Choosing a shelf on the web filters in place** — the address does not
  change. Correct. **On iOS it navigates instead**, to the shelf's own screen.
  The two surfaces genuinely differ here; neither is the other one broken.
- A **Smart** shelf fills by rule, so being unable to add a book to it by hand
  is correct on every surface.
- Your display name is denormalised into the wishlist shelf's name. It has been
  observed updating in the same render as a profile save, so a *stale* name
  there is worth journalling as a real observation rather than shrugging off.

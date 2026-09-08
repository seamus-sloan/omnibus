# Creating a shelf

| | |
|---|---|
| **Runs** | on its own |
| **Owner-only** | no; shelves are per-user |
| **Surfaces** | web (create, select, edit), iOS (create, fill, edit, delete) |
| **Actions** | `shelf.create`, `shelf.select`, `shelf.edit`, `shelf.delete` |

Make a shelf and confirm it exists, is yours, and behaves when selected. Then
change it, and — on iOS, for a shelf you made in this flow — take it away
again. **The web has no shelf delete**, the same way it has no add-to-shelf:
a web-made shelf stays on the rail until an iOS reader removes it, so choose
its name as something you would be content to leave behind.

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
  **Add books** button an empty hand-picked shelf offers in its place. Your
  own shelves are listed first on the grid, after your wishlist — a new one
  appears near the top, not at the end.
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
   from a rule; a hand-picked one does not). **Make it Smart at least every
   other time on the web**, because that is the only shelf the web can fill:
   write a rule a reader would — an author you saw in the index, a genre from
   a book page — and read the **preview** the form shows before you create.
   The preview is a count ("3 of 28 match"), not a list; the titles are
   checkable only by selecting the shelf afterwards. Journal the rule and the
   count; the shelf must hold exactly the matching books once created, and a
   book that matches the rule but is missing, or one that does not and is
   present, is a finding. The web form opens with **Smart** and **Private**
   already chosen (the iOS sheet with **Manual** and Private), and the web
   Create button may read `Create · N` once a name is typed.

   **iOS only shows you back a *public* choice.** The shelf screen's meta line
   reads "N books · Manual · Public", and a private shelf simply omits that
   last part — there is no "Private" label anywhere. So on iOS you can confirm
   Public directly and Private only by the absence of the marker. If you want
   the visibility you chose to be checkable at all on that surface, choose
   **Public**; if you choose Private, journal the choice and record the
   confirmation as `uncertain` rather than claiming the app agreed with you.
5. Click **Create**.
6. **(web)** Confirm it appears in the rail, with the name, visibility, and kind
   you chose — a public shelf says PUBLIC and a private one carries no marker,
   on the web as on iOS, so Private is confirmed by absence. **(iOS)** Confirm the card appears on the Shelves grid; open it
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
11. **(web) Edit it.** Open **Edit shelf** (the pencil beside the facets) and
    change **one** thing — the name, or the visibility, or on a Smart shelf
    the rule. **iOS has no Edit shelf control** — a shelf's name and
    visibility are fixed at creation there; journal `shelf.edit` as
    `uncertain` saying so, and do not hunt. Save, and confirm the change on
    the rail or card and after a reload. On the web the form also carries a
    **Sync to Kobo** Off/On toggle: read it and **leave it Off** unless you
    were handed
    [kobo_sync.md](../kobo_sync.md). The wishlist shelf has no Edit control
    at all on the web, which is how it declines the opt-in; confirm that
    once. Journal `shelf.edit` with before and after. The audit verifies a shelf by name, so a rename must carry both:
    `old_name` for the name it had and `name` for the name it has now — the
    audit then stops looking for the old one and looks for the new one. A
    visibility or rule edit carries the unchanged `name`; the audit does not
    check visibility or rules, only that the shelf is there.
12. **(iOS only) Delete it — only a shelf you created in this flow.** The
    delete control is **Delete shelf** in the Shelves grid card's long-press
    menu, and it runs on the single tap with no confirmation — journal that
    as an observation. The card must go from the grid; pull to refresh and
    confirm it stays gone. Then confirm every book that was on it is **still
    in the library** — a shelf delete removes the shelf, never a book. Journal
    `shelf.delete` with the name; the audit pops the matching `shelf.create`
    and expects nothing. Never delete a shelf you did not create this flow,
    and never the wishlist. **On the web this step does not exist**: Edit
    shelf offers name, visibility, rules and the Kobo opt-in and nothing
    else. Do not hunt for a delete; journal that the shelf remains, and end
    the flow on the other criteria.

## Journal

`shelf.create` with the name, visibility, and kind — and for a Smart shelf the
rule and the preview count. `shelf.select` with the name and the resulting
count. `shelf.edit` with the field, before and after — and on a rename,
`old_name` beside `name`. `shelf.delete` with the name. On iOS, `shelf.add` /
`shelf.remove` with the shelf and the book uuids — those, the create, the
edit, and the delete are what the audit reconciles.

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
- A Smart shelf holds exactly the books its rule previewed.
- An edit sticks, and on iOS a delete removes the shelf and nothing else.

## Fail

- The shelf saves under a different name, visibility, or kind.
- It does not appear, or disappears after a reload.
- Selecting it shows books that do not belong to it.
- Another user's **private** shelf is visible **and you were briefed as a
  reader**. Admins see every shelf by design, so for an admin agent the
  criterion is undecidable — journal `uncertain` rather than guess. The runner
  can provision one non-admin reader per run precisely so this becomes
  decidable; if that is you, it is a real fail. (Worth reporting separately
  either way: whether every other reader's shelf carries its "BY <owner>"
  line — the web rail did in the last run; iOS cards carry none, so nothing
  there says whose shelf is whose.)
- **On iOS:** removing a book from a shelf deletes the book. High severity.
- Deleting a shelf (iOS) deletes a book. High severity.
- A Smart shelf's preview and its contents disagree.

## Sharp edges

- **Choosing a shelf on the web filters in place** — the address does not
  change. Correct. **On iOS it navigates instead**, to the shelf's own screen.
  The two surfaces genuinely differ here; neither is the other one broken.
- A **Smart** shelf fills by rule, so being unable to add a book to it by hand
  is correct on every surface.
- Your display name is denormalised into the wishlist shelf's name. It has been
  observed updating in the same render as a profile save, so a *stale* name
  there is worth journalling as a real observation rather than shrugging off.

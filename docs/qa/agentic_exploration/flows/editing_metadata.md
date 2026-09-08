# Editing a book's metadata

| | |
|---|---|
| **Runs** | inside browsing_book_details |
| **Owner-only** | **no** — but see below |
| **Surfaces** | web |
| **Actions** | `metadata.open`, `metadata.save`, `cover.replace`, `genre.edit`, `tag.edit` |

Metadata edits are **library-wide**: every user sees them, and they outlive
reindexing. Any agent may make them, which is deliberate — but it means the
audit cannot verify them, because two agents editing one book leaves no single
correct answer. Your journal entry is still the record of what you meant.

Because this is shared and unverifiable, **be conservative**: change one or two
fields, make the change obviously yours, and never blank a field that had
content.

## Three doors

Metadata can be edited from three places, and the point of rotating between
them is that each has been wrong on its own before:

- **The editor page**, opened from the book's detail page. Every field; a
  Save with an edited-field count. Steps 1–7 below describe it.
- **The detail page's chip editors.** The `+` beside the genres and the `+`
  beside the tags on the detail page open an inline editor for that one
  list, saved on the spot. Use these for a genre or tag edit at least
  sometimes instead of the editor page; the criteria are the same, and the
  edited chip must appear in the list without a reload.
- **The library table's inline cells.** In table view, clicking a title,
  author, series, tags, genres, published or language cell opens an editor in
  place; Enter saves, Escape discards. Occasionally make your one edit here —
  then open the detail page and confirm it agrees, and confirm the **sort**
  still holds if the column you edited is the one the table is sorted on
  (see [sorting_the_library.md](sorting_the_library.md): the displayed value
  and the sort key must agree after an edit).

Whichever door, the rules below about what to write and what to journal are
the same.

## Steps

1. From a book's detail page, open its metadata editor.
2. Read the current values before changing anything, and journal them.
3. Change one or two fields. Good candidates: subtitle, series name or number,
   publisher, tags, genres, description. Make it a **plausible** value — a real
   publisher, a genre the book could actually be. Fill a field that is empty;
   where one already has content, add to it rather than replacing it — a
   `maritime` tag beside the existing tags, not instead of them.

   Do **not** sign your edit. A title reading `Foo (ed. agent-3)` is
   library-wide and permanent — every reader sees it, on a book nobody owns —
   and your journal entry already records that you made the change. See *What
   you type into the app* in [start.md](../start.md).
4. Occasionally replace the cover image instead. Use **that book's own**
   `cover.jpg` sidecar from the corpus — another book's cover would make the
   shared library worse, which this flow tells you not to do. Note that the
   cover is written **immediately** on picking it, before you press Save, and
   the editor's Discard link cannot undo it.
5. Save. Watch for a confirmation and for the edited-field count to match what
   you actually changed.
6. Return to the detail page and confirm the new values are shown.
7. Reload and confirm they are still shown.

## Journal

`metadata.open` with the uuid and the **before** values of everything you are
about to touch. `metadata.save` with the field names and their before and after
values, and `door` — `editor`, `chips` or `table` — so a finding names where
the edit was made. `genre.edit` / `tag.edit` when you used a chip editor, with
before and after. `cover.replace` with the uuid and the source filename. All
of these are out of the audit's scope, by design: the journal is the record.

The before values matter more here than anywhere else: with several agents
editing freely, the only way to reconstruct what happened is from each agent's
own record of what it found and what it left.

## Pass

- The editor loads with the book's current values, not empty or stale ones.
- The count of edited fields matches the number you changed.
- Saving confirms, and the detail page shows the new values.
- The values survive a reload.
- A replaced cover appears everywhere the book does, allowing a moment for
  thumbnails.

## Fail

- The editor opens with another book's values.
- Saving reports success but nothing changes.
- An edit made through one door is not visible through another — a table
  cell edit the detail page does not show, or a chip edit the editor page
  does not carry.
- A table cell edit changes the displayed value but the column no longer
  sorts on it.
- A field you did not touch changes.
- The save errors, or hangs past thirty seconds.
- A replaced cover appears on a different book.

## Sharp edges

- **Do not blank fields.** Overrides are the authority in this app and an empty
  override is not the same as no override.
- A field reverting between two of your visits is most likely another agent,
  not a bug. Only call it a failure if it reverts within your own flow, with no
  save in between.
- Thumbnails are cached, so a new cover may take a moment to propagate to the
  library grid. Wait and re-check before reporting it.
- Genres are stored as an override like any other edit — editing them marks the
  book as having overrides, which is expected.
- The description accepts a paragraph of prose. Editing it is fine; blanking
  it is not, and a description that arrived as raw HTML from the file is a
  finding about the file's import, not something to fix by hand.
- On iOS there is no editor of any kind; this subflow is web-only, and the
  runner will not hand it to the iOS agent.

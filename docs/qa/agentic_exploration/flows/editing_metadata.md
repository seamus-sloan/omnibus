# Editing a book's metadata

| | |
|---|---|
| **Weight** | 25% of a details flow |
| **Owner-only** | **no** — but see below |
| **Surfaces** | web |
| **Actions** | `metadata.open`, `metadata.save`, `cover.replace` |

Metadata edits are **library-wide**: every user sees them, and they outlive
reindexing. Any agent may make them, which is deliberate — but it means the
audit cannot verify them, because two agents editing one book leaves no single
correct answer. Your journal entry is still the record of what you meant.

Because this is shared and unverifiable, **be conservative**: change one or two
fields, make the change obviously yours, and never blank a field that had
content.

## Steps

1. From a book's detail page, open its metadata editor.
2. Read the current values before changing anything, and journal them.
3. Change one or two fields. Good candidates: title suffix, subtitle, series
   name or number, publisher, tags, genres, description. Append rather than
   replace where you can — `Foo (ed. agent-3)` beats overwriting `Foo`.
4. Occasionally replace the cover image instead, using a file from the corpus.
5. Save. Watch for a confirmation and for the edited-field count to match what
   you actually changed.
6. Return to the detail page and confirm the new values are shown.
7. Reload and confirm they are still shown.

## Journal

`metadata.open` with the uuid and the **before** values of everything you are
about to touch. `metadata.save` with the field names and their before and after
values. `cover.replace` with the uuid and the source filename.

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

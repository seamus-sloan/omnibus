# F3.1 — Shelves

**Phase 3 · Personalization** · **Priority:** P1

Named subsets of the library — auto-gathered by a rule (Smart) or curated by
hand (Hand-picked) — surfaced in a left rail that replaces the old home-page
filters.

> Supersedes the former **F3.1 Libraries with metadata filters** and **F3.5
> Shared shelves**, which are folded into this single concept.

## Objective

Give users a first-class way to view subsets of their library. A **shelf** is a
named collection of books shown in a left rail on the Library page; selecting
one scopes the grid/table to that subset. The rail's **All books** item is the
way back to the full library. Two kinds, chosen at creation:

- **Smart** — membership is computed from a structured rule (a `match any/all`
  set of `field op value` conditions over normalized metadata). Updates itself
  as the library changes. Marked with a cog icon.
- **Hand-picked** — membership is an explicit, ordered list of books the user
  adds by multi-selecting in the library, dragging onto the shelf, or picking in
  the create dialog.

A book can live on any number of shelves at once.

## User / business value

The core v1 differentiator (v1 #3). Self-hosters with thousands of books need
slice-and-dice beyond a flat list. Folding the old filter-libraries and shared
shelves into one concept removes a confusing distinction (filter-defined vs
hand-curated) — they are now just the two kinds of one thing, and either can be
shared with the whole instance.

## Functional scope

**The rail (replaces home-page filters).**

- A left rail on the Library page lists **All books** (with a total count) and
  every shelf the current user can see. The previous filter row on the home
  page is removed.
- Each shelf row shows its kind marker (cog for Smart, an accent swatch for
  Hand-picked), name, book count, and a visibility icon (lock = Private,
  people = Public). A footer note reminds that Smart shelves update on their own.
- Header affordances: a **＋ New shelf** button and a collapse/minimize toggle.

**Creating a shelf (modal with a Smart / Hand-picked toggle).**

- **Smart**: name field, the Smart/Hand-picked toggle, a `Match [Any|All] of
  these` rule builder with `field op value` condition rows (＋ Add condition),
  the visibility control, and a **live preview** pane showing "N of M match"
  with the matching covers as conditions change.
- **Hand-picked**: name field, the toggle, a searchable grid of the user's
  library to multi-select from, an "On this shelf · N" running list, the
  visibility control, and a **Create · N** action.
- The kind is fixed at creation (no in-place Smart↔Hand-picked conversion in
  v1 — see open questions).

**Smart-shelf rule vocabulary** (mirrors the [F1.5 advanced search](1-5-advanced-search.md) query shape):

- Fields: `tag`, `author`, `series`, `rating`, `status`, `format`, `year`.
- Operators: `is`, `is not`, `is at least` (≥), `includes`.
- `match` is `any` (OR) or `all` (AND) across conditions. No free-text DSL in
  v1 — the structured builder is the only authoring surface.

**Shelf detail pages.**

- Shared header: back-to-**All books** link, kind badge, visibility badge,
  shelf name, and a `⋯` menu (rename, change visibility, delete).
- **Smart**: rule chips ("Tag is Fantasy"), an **Edit rule** action, an
  auto-sorted grid with a sort control. Membership is read-only — it is the
  rule's output.
- **Hand-picked**: a blurb, **＋ Add books**, a drag-to-reorder grid (each cover
  carries a drag handle), and a dashed **Add books** tile.

**Adding books in bulk (multi-select → floating bar).**

- Hovering a cover in the library reveals a select control; selecting books
  enters a selection mode that surfaces a floating bottom action bar reading
  "N books selected" with **＋ New shelf** and **Add to shelf ▾**, plus a Done
  exit. This moves many books onto a shelf at once.
- **Drag & drop**: a book (or an active multi-selection) can be dragged onto a
  shelf row in the rail to add it there.

## Technical considerations

- **Smart rules operate on normalized columns** from
  [F0.1](0-1-schema-refactor.md) (`authors.id`, `tags.name`, `series`,
  `rating`, derived read `status`, `book_files` format, `year`) — not JSON
  blobs. The condition shape is the same `field/op/value` structure
  [F1.5 advanced search](1-5-advanced-search.md) defines; build it once and
  share it so neither feature reinvents the other's rule format.
- **Storage.** A `shelves` table —
  `(id, owner_user_id, kind 'smart'|'manual', name, description, visibility
  'private'|'public', match 'any'|'all' NULL-for-manual, accent, icon,
  position, created_at, updated_at)`. Smart membership lives in a
  `shelf_rules` child table `(shelf_id, field, op, value)` (the rule rows the
  former F3.1 called `library_filter_rules`). Hand-picked membership lives in
  `shelf_books` `(shelf_id, book_uuid, position, added_by_user_id, added_at)`.
- **Membership is uuid-soft-referenced.** Per [rule 06](../../.claude/rules/06-migrations.md),
  `shelf_books` references the durable `book_uuid` (no cascading `book_id` FK),
  resolved through `resolve_book_id_by_uuid` so a reindex / scan-root repoint /
  format merge keeps a hand-picked shelf intact and a removed file only ghosts
  its entry. A book on N shelves is N `shelf_books` rows. `position` backs
  drag-to-reorder.
- **Naming sidesteps the `libraries` collision.** The physical scan-root
  registry keeps the `libraries` name; this user-facing concept is `shelves`,
  resolving the table-name clash flagged in the db review.
- **Visibility & authorization** ties into
  [F0.7 per-route authorization](0-7-route-authorization.md). A shelf is
  visible to its owner, to **all admins**, and — when `visibility = 'public'`
  — to **every user**. Every list/read path filters with
  `owner_user_id = :me OR visibility = 'public' OR :me_is_admin`. Membership
  lookups for the rail run on every Library page load, so keep the
  visible-shelves query cheap (indexed on `owner_user_id`, `visibility`).
- **Roles are admin / user only.** There are no per-shelf viewer/contributor
  roles. An admin's own shelves behave exactly like a user's (Private =
  owner + admins; Public = everyone); admins additionally can view any shelf
  for moderation.

## Out of scope (dropped from former F3.5)

- **Anonymous link-sharing.** No long-random read tokens / unauthenticated
  shelf URLs.
- **Per-shelf membership & roles.** No `shelf_members`, no
  viewer-vs-contributor permissions, no per-member `finished_by` tracking.
  Sharing is the binary Private/Public flag above.

## Dependencies

- [F0.1 Schema refactor](0-1-schema-refactor.md) — normalized columns for
  Smart rules.
- [F0.3 Auth](0-3-auth.md) — owner / visibility need multi-user.
- [F0.7 Per-route authorization](0-7-route-authorization.md) — enforces the
  visibility rule and admin-sees-all.
- [F1.5 Advanced search](1-5-advanced-search.md) — shares the structured
  condition shape with Smart-shelf rules.

## Acceptance criteria

- A user creates a Smart shelf from the rule builder; its membership matches the
  rule and updates automatically when a qualifying book is added/changed.
- A user creates a Hand-picked shelf, then adds books to it three ways:
  the create dialog, library multi-select → "Add to shelf", and drag-and-drop
  onto the rail row. The same book can appear on more than one shelf.
- Hand-picked books reorder by drag; the order persists.
- A Private shelf is visible only to its owner (plus admins); a Public shelf is
  visible to every user. Flipping visibility changes who sees it immediately.
- An admin can view another user's Private shelf; a non-admin cannot.
- A hand-picked shelf survives a reindex / scan-root repoint with its membership
  intact (uuid soft-reference), and a removed book simply drops from the shelf.
- The home page shows the shelf rail and no filter row.

## Open questions

- **OPDS + Kobo: whose shelves does a device see?** Presumably the
  authenticating user's visible set (own + public, and all if admin). Document
  explicitly when [F4.1](4-1-kobo-sync.md) / [F4.2](4-2-opds.md) ship.
- **Smart ↔ Hand-picked conversion.** v1 fixes kind at creation. Worth
  revisiting if users ask to "freeze" a smart shelf into a hand-picked one.
- **Shelf ordering in the rail.** `position` is reserved; whether users can
  manually reorder shelves (vs. a fixed sort) is deferred.
- **Sort options per shelf.** Smart shelves are auto-sorted with a sort
  control; the default and the full set of sort keys are a UI decision.

## Related

- **Design mockups** — [Omnibus Claude Design project](https://claude.ai/design/p/22f5445b-4ec1-4865-9960-c9e48d0ff2e7?file=Omnibus+Design.html)
  (`screens/shelves.jsx`). Source of truth for the rail, create modal, and
  multi-select flows described above. Requires access to the design project.
- [Atrium design system](../design/atrium-design-system.md).

---

[← Back to roadmap summary](0-0-summary.md)

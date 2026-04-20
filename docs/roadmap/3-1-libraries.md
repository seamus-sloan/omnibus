# F3.1 — Libraries with metadata filters

**Phase 3 · Personalization** · **Priority:** P1

User- and admin-defined collections filtered by metadata rules.

## Objective

Let users (and admins, with different scope) define named "libraries" as saved filter rules against book metadata — e.g. "Sci-fi I haven't read," "Audiobooks by author X," "Tagged #work." Libraries appear in navigation and scope every browse/search operation inside them.

## User / business value

The core v1 differentiator (v1 #3). Self-hosters with thousands of books need slice-and-dice beyond a flat list; saved filters are the primary mechanism.

## Technical considerations

- Filter rules operate on **normalized columns** from [F0.1](0-1-schema-refactor.md) (`authors.id IN (…)`, `tags.name = ?`), not JSON blobs — significantly simpler than the v1 plan which predated the schema refactor.
- Admin-scoped libraries are visible to all users; user-scoped libraries are private.
- Rule persistence: a `libraries` table + `library_filter_rules` child table with `(library_id, field, op, value)` rows. Keep the DSL simple; expression trees can wait.

## Dependencies

- [F0.1 Schema refactor](0-1-schema-refactor.md).
- [F0.3 Auth](0-3-auth.md).

## Changes from v1

- Filter DSL operates on normalized columns instead of JSON blobs.
- Admin-vs-user scoping preserved from v1.

## Open questions

- OPDS + Kobo: whose libraries does a device see? Presumably the authenticating user's visible set (admin + own). Document explicitly when [F4.1](4-1-kobo-sync.md) / [F4.2](4-2-opds.md) ship.

---

[← Back to roadmap summary](0-0-summary.md)

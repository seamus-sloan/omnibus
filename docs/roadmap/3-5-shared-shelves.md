# F3.5 — Shared shelves

**Phase 3 · Personalization** · **Priority:** P3

Multi-user shelves for family libraries, book clubs, or partner reading lists.

## Objective

A new "shelf" concept that sits between [F3.1 Libraries](3-1-libraries.md) (filter-defined collections) and an ad-hoc bookmark. A shelf is an explicit list of book IDs owned by one user but optionally shared with others (read-only or read-write). Common use cases: "Family library" (parents curate, kids browse), "Book club: Q3 2026" (members add picks, mark finished), "Birthday gift list" (shared with a partner, items get checked off).

## Objective scope

- New `shelves` table: `id, owner_user_id, name, description, visibility (private/link/users), created_at`.
- New `shelf_books` table: `shelf_id, book_id, added_by_user_id, added_at, finished_by` (jsonb of user ids → timestamp).
- New `shelf_members` table: `shelf_id, user_id, role (viewer/contributor)`.
- New routes: `/shelves`, `/shelves/:slug`. Book-detail page gains a "Add to shelf" action.
- Permission model: viewer sees the shelf; contributor can add/remove books; owner can rename/delete.

## User / business value

The single feature self-hosted users routinely ask for that no commercial reader app offers. Once a family has a shared library, churn is near-zero — switching cost is "rebuild everyone's annotations on this shelf."

## Technical considerations

- Shelf membership lookups must be cached or denormalized — every page-load checks "can user X see shelf Y."
- Anonymous link-sharing (visibility=`link`) issues a long-random URL token, no auth required for read. Useful for sending a curated list to a non-user.
- Doesn't replace [F3.1 Libraries](3-1-libraries.md). Libraries are filter-defined ("everything tagged Fantasy"), shelves are hand-curated ("our Hugo Award reading list").

## Dependencies

- [F0.3 Auth](0-3-auth.md) — needs multi-user.
- [F3.1 Libraries](3-1-libraries.md) — clarifies the boundary between the two concepts before either ships.

## Acceptance criteria

- Owner creates a shelf, adds books, invites a second user as contributor.
- Contributor sees the shelf in their `/shelves` index, can add a book, finishes one.
- Owner sees the contributor's additions and finish-marks in real time (poll, not push, in v1).
- Revoking a member removes the shelf from their UI immediately.

## Related

- [Atrium design doc](../design/atrium-design-system.md).

---

[← Back to roadmap summary](0-0-summary.md)

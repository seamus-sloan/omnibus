# F4.2 — OPDS feed

**Phase 4 · Device sync** · **Priority:** P2

OPDS 1.2 (Atom) + 2.0 (JSON) catalogs for non-Kobo readers.

## Objective

Serve an OPDS Atom catalog at `/opds/*` for apps that speak OPDS (KOReader, Moon+ Reader, Marvin, Calibre Companion) and a parallel OPDS 2.0 JSON catalog for modern clients.

## User / business value

Covers every reading app that isn't Kobo or Kindle. OPDS is the portable, interoperable path — niche but durable.

## Technical considerations

- Route layout mirrors Calibre-Web's for client compatibility (see [calibre-inspection §2](../calibre-inspection/2-feature-inventory.md), [§6](../calibre-inspection/6-api-surface.md)): `/opds`, `/opds/osd`, `/opds/search`, `/opds/new`, letter-indexed author/series/category browses, per-entity endpoints, download, cover.
- OPDS 2.0 is trivial given the Kobo sync endpoint's JSON shape is adjacent — ship both.
- Libraries visible to the authenticated user scope every feed (see [F3.1 open question](3-1-libraries.md#open-questions)).

## Dependencies

- [F0.3 Auth](0-3-auth.md).

## Changes from v1

- OPDS 2.0 added — v1 specified 1.2 only.

---

[← Back to roadmap summary](0-0-summary.md)

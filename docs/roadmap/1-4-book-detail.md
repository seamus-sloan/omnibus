# F1.4 — Book detail page

**Phase 1 · Browse & discovery** · **Priority:** P1

Full metadata, read/listen CTAs, breadcrumb, format switcher.

## Objective

A single per-book landing page showing full metadata (title, authors, series, description, tags, identifiers), all available formats with per-format actions (read / listen / download / send-to-Kindle), and placeholder slots for ratings and suggestions that ship filled later.

## User / business value

The destination for every search result, cover-grid click, and external OPDS link. Also the host for Ratings ([F3.2](3-2-ratings-journaling.md)) and Suggestions ([F3.3](3-3-suggestions.md)) — those slots exist in Phase 1 markup and fill with content in Phase 3.

## Technical considerations

- One query per book load — all m2m relations joined + aggregated, per [recommendation #5](../calibre-inspection/7-recommendations.md).
- Format switcher is the UI for the `books` / `book_files` split from [F0.1](0-1-schema-refactor.md). A work with epub + m4b shows both; clicking "Listen" loads the audiobook player, "Read" loads the epub reader, "Send to Kindle" picks the epub.
- Breadcrumb uses the author/series relations from [F0.1](0-1-schema-refactor.md) — not the filesystem path.

## Dependencies

- [F0.1 Schema refactor](0-1-schema-refactor.md).

## Changes from v1

- Detail page is explicitly the host for Ratings and Suggestions markup slots from day one, even though the content lands in Phase 3.

---

[← Back to roadmap summary](0-0-summary.md)

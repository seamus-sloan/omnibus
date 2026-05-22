# F1.10 — Palette ↔ Discovery page links

**Phase 1 · Browse & discovery** · **Priority:** P3

Link search-palette entity results directly to F1.8 discovery pages once those routes exist.

## Objective

The F1.5 search palette currently navigates author/series/tag results to the landing page with a `SearchQuery` filter applied (e.g. `author:kuang`). This is a stopgap. Once the F1.8 discovery pages ship (`/authors/:slug`, `/series/:slug`, `/tags/:name`), the palette should route to those pages instead — giving the user a dedicated page for the entity they selected, not just a filtered book list.

## Dependencies

- **F1.5 Search palette** (this branch) — provides the palette component and `PaletteAuthorHit` / `PaletteSeriesHit` / `PaletteTagHit` result types.
- **F1.8 Discovery pages** — provides the routes to link to.

## Scope

1. Update `search_palette.rs` `navigate_to_item` so `FlatItem::Author` pushes `Route::AuthorDetail { slug }` instead of setting `SearchQuery`.
2. Same for `FlatItem::Series` → `Route::SeriesDetail { slug }` and `FlatItem::Tag` → `Route::TagDetail { name }`.
3. The palette hit types may need a `slug` field added (currently carry `id` + `name`). Coordinate with F1.8's slug strategy — if F1.8 uses `id`-based routes the change is trivial; if slug-based, `search_palette` in `db/src/queries.rs` needs to join `authors.slug`.
4. Update `search-palette.spec.ts` navigation tests to assert the new routes.

## Effort

Low — a few lines of route-push logic and a slug join. The majority of the work is in F1.8 itself.

## Status

Queued — blocked on F1.8.

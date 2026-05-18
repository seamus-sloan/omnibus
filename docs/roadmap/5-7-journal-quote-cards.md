# F5.7 — Journal entries & quote cards

**Phase 5 · Admin & hygiene** · **Priority:** P2

Markdown journal entries with embedded images, saved highlights, and a shareable quote-card composer.

## Objective

Extend [F3.2 Ratings & journaling](3-2-ratings-journaling.md) from "free-text notes" to a real journaling surface:

- **Markdown journal entries** — one or many per book, with date and reading-progress percentage attached. Editor is a vertically-split markdown source / live preview. Image upload via paste or drag-drop; images stored as attachments under `<library>/.journal/`.
- **Highlights** — passages saved while reading (eventually piped from [F2.2 EPUB reader](2-2-epub-reader.md); manually entered for now). Each highlight has page number and date.
- **Quote card composer** — turn any highlight into a shareable PNG. Composer picks a template (typographic, on-cover, minimal), color palette derives from the book's `accent_color`, output is a 1200×1200 PNG suitable for social.

## Objective scope (out)

- Public/social sharing surface beyond "download the PNG." A future initiative could host quote URLs.
- Cross-book journal threads (a journal entry referencing multiple books).
- Reading-progress event hookup — comes for free from [F2.1](2-1-progress-sync.md).

## User / business value

The journal is what differentiates Omnibus from a fancy file browser. Readers who keep notes on what they read are the most engaged users; once a journal lives here, it doesn't leave.

## Technical considerations

- New `journal_entries` table: `id, book_id, user_id, body_md, progress_pct, written_at, attachments (jsonb of paths)`.
- New `highlights` table: `id, book_id, user_id, page, quote, saved_at`.
- PNG composition server-side via `imageproc` + `usvg` (render SVG template, rasterize). Per-card cost ~50 ms.
- Quote card template SVGs live in `assets/quote-cards/` — each is a tokenized SVG that the renderer substitutes `{{quote}}`, `{{author}}`, `{{accent}}`, etc. into.
- Editor uses `pulldown-cmark` for the live preview pane. No collaborative editing.

## Dependencies

- [F3.2 Ratings & journaling](3-2-ratings-journaling.md) — this is the v2 evolution of that feature.
- [F1.7 Atrium design system](1-7-atrium-design-system.md) — composer UI primitives + `accent_color` for templates.
- [F2.2 EPUB reader](2-2-epub-reader.md) — auto-highlight pipe (optional; manual entry works without it).

## Acceptance criteria

- Book-detail page hosts "Journal entries" and "Highlights" sections, inline.
- New entry composer opens in a modal or full-screen sheet; saves on close.
- Quote card composer renders a 1200×1200 PNG with the chosen template; clicking download saves the file.

## Related

- [Atrium design doc](../design/atrium-design-system.md).

---

[← Back to roadmap summary](0-0-summary.md)

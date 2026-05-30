# F2.4 — Reader experience

**Phase 2 · Reading & listening** · **Priority:** P2

The full designed reading experience layered on the barebones [F2.2](2-2-epub-reader.md) reader.

## Objective

Build the complete designed reading experience on top of the barebones [F2.2](2-2-epub-reader.md) reader: immersive top/bottom chrome, a typographic reading column, a highlight→quote lifecycle (select → highlight in palette colors → promote to a quote card), a selection popover, table-of-contents + notes drawers, a progress ribbon, and per-reader typography (font family / size / line-height) — across night / paper / sepia themes.

## User / business value

F2.2 proves a book can be read in the browser; F2.4 makes reading something a user *wants* to do in Omnibus. Immersive chrome, real typography, and the highlight→quote flow are the difference between parity-with-Calibre-Web and a reader people prefer.

## Technical considerations

- Builds on F2.2's `epub.js` integration.
- Highlights / annotations map to `epub.js` CFI-range annotations so they ride F2.2's position model.
- Quote cards connect to [F5.7 Journal & quote cards](5-7-journal-quote-cards.md).
- Reuse the app Theme (paper = Light, night = Dark, Sepia).
- Visual source of truth: the Omnibus design package (`screens/reader.jsx`, `reader-screens.jsx`, `mobile-states.jsx`).

## Dependencies

- [F2.2 In-browser epub reader](2-2-epub-reader.md).
- [F2.1 Progress sync service](2-1-progress-sync.md).
- Relates to [F5.7 Journal & quote cards](5-7-journal-quote-cards.md).

## Risks

- Scope: this is a large, design-heavy surface.
- `epub.js` annotation-API limits on complex EPUB 3 layouts.

---

[← Back to roadmap summary](0-0-summary.md)

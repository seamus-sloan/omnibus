# F2.4b — Reader interactive features

**Phase 2 · Reading & listening** · **Priority:** P3

Follow-on to [F2.4 Reader experience](2-4-reader-experience.md) — the interactive, data-driven features that sit on top of the cosmetic reader chrome.

## Status

Shipped across a stacked train: selection-popover Note/Copy/Quote/Share
actions, the text-anchored note composer, the Contents and color-filterable
Highlights & notes drawers, in-book search, the typeface/spacing/margins/justify
controls plus the single/two-page **Page view** toggle, reader bookmarks (on the
unified bookmarks backend, position = CFI), and the quote-card lifecycle with PNG
export (a bespoke canvas renderer, not html2canvas — the card is a fixed layout).
Highlights now persist the selected text (migration 0030) so drawers and quote
cards render the passage. Per-book accent feeds the quote card's default
background. "Open in composer →" is stubbed until [F5.7](5-7-journal-quote-cards.md).

## Objective

Ship the interactive reader features that require new data models, epub.js annotation APIs, and backend support: highlights with palette-color selection, the highlight→quote lifecycle (select → highlight → promote to quote card), text-anchored notes, right-side drawers (table of contents, highlights & notes), in-book search, functional typography controls (typeface, spacing, margins, justify), bookmarks, and per-book accent colors.

## User / business value

F2.4 delivers the visual shell — a reader that *looks* right. F2.4b makes it *work* right: users can highlight passages, take notes, produce shareable quote cards, navigate via table of contents, search within a book, and tune the reading experience to their preference. These are the features that turn a viewer into a daily-use reader.

## Features

### Highlight → quote lifecycle

1. **Text selection** — detect epub.js selection events via the rendition `selected` callback; surface a floating popover above the selection.
2. **Selection popover** — five color swatches (amber / green / blue / rose / violet, all oklch at consistent lightness/chroma), plus action buttons: Note, Copy, Quote card, Share.
3. **Highlight persistence** — store highlights as `(book_uuid, epub_cfi_range, color, created_at)` rows in a new `highlights` table. Map to epub.js CFI-range annotations so they survive page turns and re-opens.
4. **Note composer** — anchored to a highlight, opens below the highlighted span. Markdown-capable text area with Cancel / Save note actions.
5. **Quote card panel** — right-side drawer; converts a highlight into a styled card (book accent background, attribution, "OMNIBUS · QUOTE" header). Background color picker (5 presets + custom hex via `<input type="color">`), aspect ratio selector (1:1, 4:5, 9:16, 3:4), Download PNG action.

### Drawers

6. **Table of contents drawer** — right-side panel reading `book.navigation.toc` from epub.js. Sections grouped by part, current chapter highlighted with an accent border. Clicking a chapter navigates to it.
7. **Highlights & notes drawer** — right-side panel listing all highlights for the current book, color-filterable by the 5 palette colors. Each entry shows the quoted text, color indicator, note preview (if present), page reference, and date. Quick actions: Quote card, Copy.

### In-book search

8. **Search** — triggered from a search icon button in the top chrome. Uses epub.js `book.spine.find(query)` to search across chapters, surfaces results in a dropdown or drawer with chapter-grouped matches.

### Functional typography controls

9. **Typeface** — three font options (Editorial = Instrument Serif, Classic = EB Garamond, Modern = Georgia). Apply via `rendition.themes.font()`.
10. **Line spacing** — three presets (Tight / Cozy / Airy) mapped to epub.js rendition line-height overrides.
11. **Margins** — three presets (Narrow / Normal / Wide) adjusting the reading column max-width.
12. **Justify text** — toggle epub.js rendition text-align between `left` and `justify`.
13. **Page view** — single page vs two-page spread, driven by epub.js `rendition.spread` setting.

### Bookmarks

14. **Bookmark** — save the current page position as a named bookmark in a `bookmarks` table (schema already laid out in migration 0013). Surface in a bookmarks drawer or merged into the highlights panel.

### Per-book accent

15. **Per-book accent color** — derive or store an accent color per book (from cover palette or manual override) and pass it as `--accent` on the reader surface. Currently defaults to the global amber accent.

## Technical considerations

- Highlights / annotations map to `epub.js` CFI-range annotations so they ride F2.2's position model.
- The `bookmarks` and `reading_sessions` tables are already laid out in migration `0013_reading_progress.sql` — no new migration needed for bookmarks.
- Highlights need a new `highlights` table (migration) + CRUD in `db/`.
- Quote cards connect to [F5.7 Journal & quote cards](5-7-journal-quote-cards.md).
- Selection popover requires JS interop: epub.js fires a `selected` event with a CFI range; the glue must relay this to Rust.
- Drawer UI can reuse the existing listen page's drawer pattern (chapters drawer, bookmarks drawer).

## Dependencies

- [F2.4 Reader experience](2-4-reader-experience.md) (cosmetic chrome — must ship first).
- [F2.2 In-browser epub reader](2-2-epub-reader.md) (epub.js integration).
- [F2.1 Progress sync service](2-1-progress-sync.md) (position persistence).
- Relates to [F5.7 Journal & quote cards](5-7-journal-quote-cards.md) (quote card output).

## Risks

- epub.js annotation API has known limitations with complex EPUB 3 layouts (reflow, fixed-layout).
- Search performance on large books may require debouncing or background workers.
- Quote card PNG export requires canvas rendering (html2canvas or similar) in the browser.

---

[← Back to roadmap summary](0-0-summary.md)

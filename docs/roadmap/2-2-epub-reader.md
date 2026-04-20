# F2.2 — In-browser epub reader

**Phase 2 · Reading & listening** · **Priority:** P1

epub.js integration, themed to Omnibus, progress synced via [F2.1](2-1-progress-sync.md).

## Objective

Open any epub in the browser with font-size, theme, and pagination controls. Persist reading position as an EPUB CFI and round-trip it through the progress service so a user can switch device mid-book.

## User / business value

"I can read this right now without downloading" is a core self-hosted library promise. Calibre-Web has it; we need parity.

## Technical considerations

- `epub.js` is the de-facto library (Calibre-Web also uses it — see [calibre-inspection §2](../calibre-inspection/2-feature-inventory.md)). Vendor or pin via npm — do not CDN-load.
- CFI string is the canonical position; write through F2.1 on debounce + on unload.
- Themes: light, sepia, dark. Dioxus context provides theme; the reader iframe is styled via `epub.js` themes API.

## Dependencies

- [F2.1 Progress sync service](2-1-progress-sync.md).

## Risks

- epub.js rendering quirks on complex EPUB 3 layouts. Mitigation: accept parity with Calibre-Web's known-good reader behavior for v1.0.

---

[← Back to roadmap summary](0-0-summary.md)

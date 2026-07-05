# F6.2 — Mobile epub reader

**Phase 6 · Mobile** · **Priority:** P2

Read epubs inside the native mobile app, reusing the F2.1 progress model and the F2.2 file-serving route.

## Objective

Let a user read any epub inside the native `mobile/` app, reusing the shared
reader UI and keeping reading position canonical as an EPUB CFI so it
round-trips through F2.1 across web and mobile.

## Status

- **Mobile reader chrome (shipped).** The reader is one shared Dioxus
  component; the native shell renders the same tree in a WebView. A
  phone-width breakpoint in `frontend/assets/atrium.css` (`@media
  (max-width: 600px)`, the "Reader on phone widths" block) adapts the desktop
  chrome to the mobile handoff: slim top/bottom bars, no gutter page-turn
  buttons, full-width drawers (contents / search / highlights / bookmarks /
  quote card), a bottom-sheet selection popover, and a top-sheet Aa panel.
  Covered by the phone-viewport tests in
  `ui_tests/playwright/tests/flows/reader.spec.ts`.
- **Native epub streaming (remaining).** Wiring `epub.js` to actually stream
  and paginate a book on-device — a `document::eval` bridge in place of the
  web build's `wasm-bindgen` interop, plus a bearer-authed fetch of
  `GET /api/ebooks/:uuid/file` — is the open work below.

## User / business value

Reading is the core activity. Without an in-app reader, mobile is a browse-only shell and users bounce to a separate app to actually read — breaking the "everyday tool" promise of [F6.1](6-1-mobile.md).

## Technical considerations

Two candidate approaches; **no decision is locked** — prototype both early in Phase 6.

- **A. Embed a system WebView.** Host a platform WebView (WKWebView on iOS / Android WebView) for the reader screen and run the same vendored `epub.js` reader inside it. Maximum reuse of [F2.2](2-2-epub-reader.md). **Open question:** whether dioxus-native/Blitz can host an embedded platform WebView surface, or whether this needs the legacy dioxus-mobile WebView renderer or a platform-bridge crate.
- **B. Render EPUB XHTML natively through Blitz.** Blitz is itself an HTML/CSS engine: parse the epub with a Rust crate (e.g. `epub`) and feed each chapter's XHTML into the Dioxus tree — no JS. Most "Rust-everywhere", but pagination/reflow and an EPUB-CFI position model must be hand-built.

**Recommendation:** prototype both early in Phase 6; keep EPUB CFI canonical so either path round-trips through [F2.1](2-1-progress-sync.md).

## Dependencies

- [F2.1 Progress sync service](2-1-progress-sync.md).
- [F6.1 Mobile app](6-1-mobile.md).
- [F2.2 In-browser epub reader](2-2-epub-reader.md) — reuses its `/api/ebooks/:uuid/file` route via `reqwest`.

## Risks / open questions

- WebView embedding feasibility under Blitz (approach A).
- CFI fidelity in a hand-rolled Blitz renderer (approach B).

---

[← Back to roadmap summary](0-0-summary.md)

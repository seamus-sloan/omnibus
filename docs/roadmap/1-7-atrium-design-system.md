# F1.7 — Atrium design system

**Phase 1 · Browse & discovery** · **Priority:** P1

A cinematic-dark visual system (tokens, primitives, cover-derived accents) underpinning every Omnibus screen.

## Objective

Ship the Atrium **foundation**: a static `frontend/assets/atrium.css` with warm-neutral oklch tokens (dark + light), Geist / Geist Mono / Instrument Serif fonts (pulled from Google Fonts in the foundation PR; self-hosting under `frontend/assets/fonts/` is a follow-up inside this initiative for offline / airgapped installs), server-side cover-color extraction persisted as a new nullable `books.accent_color` column, a set of Dioxus component primitives (`Cover`, `TopBar`, `Stars`, `Chip`, `Button`, `SectionHead`, `FormatBadge`, `Crumb`, `ProgressBar`), and a dark/light theme toggle. Reskin the Library landing as proof of the system; every other screen is migrated in its own follow-up PR.

## User / business value

The current UI is generic dark-on-dark utility chrome. Atrium establishes the visual identity called out in the v2 vision ("Plex/Jellyfin for books") and gives every future feature a consistent kit instead of one-off CSS. Per-book cover-derived accents make the library feel curated, not auto-generated.

## Technical considerations

- Cover-color extraction runs in the existing indexer pipeline (`db::ebook::scan_ebook_library_with`). Uses `image` (already a dep) to decode + downsample, then a hue-bucket pass to pick the most saturated dominant color and convert to OKLCH with Björn Ottosson's matrix. Clamps `L` to [0.55, 0.78] and `C` to [0.06, 0.18] so the result reads consistently on both dark and light backgrounds.
- New migration `0006_books_accent.sql` adds `accent_color TEXT` plus a partial index on rows where it's null (to support a future backfill worker).
- Atrium CSS is a static file served via Dioxus's `asset!` pipeline. Fonts use `font-display: swap` so a missing font never blocks paint.
- Theme toggle: `data-theme="dark|light"` set on the `.atrium` wrapper `<div>` emitted by the `AtriumRoot` Dioxus component (not on `<html>`, to keep the swap declarative — no imperative DOM mutation from Rust). Web persists via `web_sys` `localStorage` under `omn.theme`; the persisted value is applied in a post-hydration `use_effect` so SSR markup stays deterministic. Mobile is in-memory only for the foundation; a follow-up adds disk persistence analogous to `data::token_store`.
- The Library landing migrates to Atrium primitives, but other pages keep their current markup with the legacy `STYLES` constant for now — the new CSS file is additive, not a replacement. Other pages migrate one PR each.

## Dependencies

- [F1.2 Thumbnail pipeline](1-2-thumbnails.md) — covers + accent extraction share the same indexer path.
- [F1.3 Library views](1-3-library-views.md) — the page being reskinned first.

## Acceptance criteria

- `cargo test -p omnibus-db` covers happy / corrupt / monochrome / timeout extraction paths and DB round-trip of `accent_color`.
- `/api/ebooks` returns an `accent` field (string or null).
- `dx serve --platform web -p omnibus` renders the Library landing in the Atrium look; per-book accent borders visible on covers.
- Theme toggle in the top bar flips dark↔light, persists across reload (web + mobile), and degrades gracefully when storage writes fail.
- Playwright `landing.spec.ts` and new `theme-toggle.spec.ts` pass.

## Related

- [Atrium design doc](../design/atrium-design-system.md) — full data flow, failure modes, rollback, test plan.

---

[← Back to roadmap summary](0-0-summary.md)

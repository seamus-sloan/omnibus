# F1.9 — Themes, density, and typography

**Phase 1 · Browse & discovery** · **Priority:** P3

Per-user customization of the Atrium look — sepia theme (reader-focused), density toggle (compact / comfy), and type pairing (editorial / modern / classic).

## Objective

Add user-facing tweaks beyond the dark/light toggle shipped in [F1.7](1-7-atrium-design-system.md):

- **Sepia theme** — warm paper-toned palette for in-app reading. Available everywhere but defaults on for the EPUB reader.
- **Density** — `compact` shrinks padding/gap tokens for power-user list density; `comfy` is the default.
- **Type pairing** — three named pairs (`editorial`: Instrument Serif + Geist; `modern`: Newsreader + Geist; `classic`: EB Garamond + IBM Plex Sans). Each named pair maps to additional `@font-face` declarations and a `type-*` class on the root.

Tweaks persist server-side per user (not just per device) so a user gets the same look across web and mobile.

## User / business value

Self-hosters with messy libraries vs. curated minimalists want very different densities. Readers who prefer paper-tone for hours of reading want sepia. Type pairing is a quiet but high-impact polish — `editorial` is the default; `classic` is for fans of book-typeset aesthetics; `modern` for users who find the italic serifs too "literary." The toggles are cheap to ship once the CSS plumbing exists.

## Technical considerations

- CSS plumbing already exists in [F1.7](1-7-atrium-design-system.md)'s `atrium.css` — adding sepia is one new `[data-theme="sepia"]` block. Density is a `[data-density="compact"]` block touching `--pad` and `--gap`. Type pairing is a `[data-type="modern"]` block re-defining `--serif` / `--sans`.
- New table `user_preferences` (or column on `users`) — `theme`, `density`, `type_pairing`, `default_accent`. All nullable; NULL means "use the app default."
- New endpoints: `GET/PUT /api/me/preferences`. Server hydrates them into the Dioxus context on first paint to avoid FOUC.
- Reader is the one surface that *overrides* the global theme (forces sepia by default), with an in-reader toggle to fall back to global.

## Dependencies

- [F1.7 Atrium design system](1-7-atrium-design-system.md).
- [F0.3 Auth](0-3-auth.md) — needs per-user storage.

## Acceptance criteria

- Settings page exposes the four toggles. Each saves immediately via PUT.
- Preferences hydrate on cold load (no FOUC between system default and user preference).
- Sepia/density/type changes apply globally; the reader honors its own sepia override.

## Related

- [Atrium design doc](../design/atrium-design-system.md).

---

[← Back to roadmap summary](0-0-summary.md)

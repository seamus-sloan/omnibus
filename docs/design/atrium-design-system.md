# Atrium — design system design doc

**Initiative:** [F1.7 Atrium design system](../roadmap/1-7-atrium-design-system.md). **Phase:** 1 — Browse & discovery. **Status:** in progress.

A cinematic-dark visual direction for Omnibus, sourced from a Claude Design handoff. The design system spans tokens, primitives, and a per-book cover-derived accent. This doc covers only the **foundation delivery** — tokens + primitives + Library reskin. Every other Atrium screen is tracked as its own roadmap initiative (see §9).

---

## 1. Context

The user mocked **Atrium** in Claude Design, then exported a handoff bundle (HTML/CSS/JSX prototype) covering nine sections: Library, Book Detail, Discovery (Search/Author/Series/Tag-cloud), Listening (Player), Journal & quote cards, Stats, Shared shelves, Setup/Admin/Metadata, and Mobile. The visual direction:

- Warm-neutral oklch tokens with three themes (dark / light / sepia)
- Type system: Instrument Serif (display) + Geist (UI) + Geist Mono (metadata)
- Per-book cover-derived accent color, with stylized "real book" CSS templates as a fallback when no cover image exists
- Editorial chrome (sticky translucent topbar, breadcrumbs, large italic display headings)

**Decisions confirmed at scoping:**

| | |
|---|---|
| Scope | **Foundation first.** Ship Atrium tokens + primitives as a reusable library and reskin one page (Library) as proof. Each remaining screen migrates in its own follow-up PR. |
| Covers | **Real cover + extracted accent.** Use existing `/api/thumbs/:id`. Extract dominant color server-side during indexing; persist on books table. Stylized templates only as fallback. |
| CSS arch | **Static `frontend/assets/atrium.css`** served by axum; referenced from index.html. Self-hosted fonts under `assets/fonts/`. |
| Tweaks | **Dark + light** with a toggle (localStorage on web; disk-persisted on mobile). Sepia / density / type pairing deferred to [F1.9](../roadmap/1-9-themes-and-density.md). |

### In scope (this delivery)

1. `frontend/assets/atrium.css` — tokens + primitive classes (cover, btn, chip, card, pbar, pct-ring, label, mono, divider) for dark + light themes. Loaded via Dioxus `asset!` from `frontend/src/lib.rs`.
2. Geist, Geist Mono, and Instrument Serif fonts pulled from Google Fonts CDN in this PR. Self-hosting under `frontend/assets/fonts/` (`@font-face`) is a follow-up inside F1.7 for offline / airgapped installs.
3. Server-side cover-color extraction in `db/src/ebook.rs` → persisted on `books.accent_color` (new column). Column name format-agnostic; values are `oklch(L C H)` strings today.
4. `EbookMetadata.accent: Option<String>` added to shared types.
5. Dioxus component primitives under `frontend/src/components/atrium.rs`: `AtriumRoot`, `Cover`, `ThemeToggle`. More primitives (`Stars`, `Chip`, `Button`, `SectionHead`, etc.) land alongside the page reskins that need them.
6. Theme toggle (dark/light) — `data-theme="dark|light"` on the `.atrium` wrapper div emitted by `AtriumRoot` (not on `<html>`). Web persists via `web_sys` `localStorage` under `omn.theme`, applied in a post-hydration `use_effect` so SSR markup stays deterministic. Mobile is in-memory only this PR.
7. `ThemeToggle` wired into `TopNav` (web).
8. Roadmap doc additions so every Atrium screen has a tracking initiative.

> The Library landing is **not** reskinned in this PR. The foundation establishes tokens + primitives + the theme infrastructure; the actual page port lands as F1.7-b. `AtriumRoot` wraps the router globally so the typography baseline (serif headings, sans body, oklch token backgrounds) applies to every page inside it — legacy pages inherit the new fonts and palette while keeping their existing layout class names. This is intentional and previews the direction; the per-page reskins replace each layout class set as they ship.

### Out of scope (deferred to follow-ups)

- Reskin of Book Detail, Search, Settings, Login. Each is one PR.
- Sepia / density / type pairing toggles — [F1.9](../roadmap/1-9-themes-and-density.md).
- All paper screens — see roadmap pages listed in §9.

---

## 2. Data flow

### 2a. Theme toggle (client-only)

The Atrium theme lives in a Dioxus `Signal<Theme>` provided at the app root by `init_theme()`. The `AtriumRoot` wrapper reads the signal and emits `<div class="atrium" data-theme="dark|light">`; the token block in `atrium.css` keys off that attribute on the **wrapper div** (not on `<html>`). Theme changes are pure re-renders — no imperative DOM mutation from Rust.

```
User clicks ThemeToggle  ──▶ Signal<Theme>::set(Theme::Light)
                              │
                              ├─ AtriumRoot re-renders with data-theme="light"
                              │       (CSS variables in atrium.css swap)
                              └─ persist_theme(Light) → web_sys localStorage
                                       .set_item("omn.theme", "light")

Cold start (web, SSR-safe):
  App() ──▶ init_theme()
              ├─ use_context_provider(|| Signal::new(Theme::Dark))   // deterministic
              └─ use_effect (web-only, runs after hydration):
                  └─ read_persisted_theme() reads localStorage
                      and signal.set() if Some(theme)
                          └─ AtriumRoot re-renders once with persisted theme

Cold start (mobile):
  App() ──▶ init_theme()
              └─ Signal::new(Theme::Dark)   // in-memory only this PR
                 (disk persistence under $HOME/.omnibus-theme is a follow-up)
```

**Shadow paths**: nil persisted value → signal stays at the SSR-rendered `Dark` default; corrupt persisted value → `Theme::from_attr` returns `None`, signal stays at default; storage write error → log warn, session-only persistence; no timeout path (all sync).

Why the post-hydration effect: SSR and the WASM client's first paint both render `data-theme="dark"`, so there is no hydration mismatch on the wrapper attribute. The `use_effect` fires only after mount and only on the WASM client (it's `#[cfg(feature = "web")]`-gated), so the persisted preference applies as a single follow-up render.

### 2b. Cover-derived accent (server-side, indexed)

```
Indexer (POST /api/rpc/settings → tokio::spawn)
   │
   ▼
db::indexer::reindex_ebook_library
   ├─▶ db::ebook::scan_ebook_library_with(opts)
   │         └─▶ for each .epub:
   │              ├─ parse OPF metadata          (existing)
   │              ├─ extract cover bytes          (existing)
   │              └─ NEW extract_accent(cover_bytes) → Option<String>
   │                       └─ image::load_from_memory + thumbnail to 32×48
   │                            ├─ hue-bucket pass (12 × 30° bins)
   │                            ├─ pick highest-weighted bucket
   │                            ├─ rgb_to_oklch (Björn Ottosson matrix)
   │                            └─ clamp L to [0.55, 0.78], C to [0.06, 0.18]
   ▼
db::queries::replace_books
   └─▶ INSERT INTO books (..., accent_color) VALUES (..., ?)

GET /api/ebooks  → SELECT ..., accent_color FROM books
                  → EbookMetadata { accent: Option<String>, .. }

Cover rendering:
   <Cover book={b} />
     ├─ if b.cover_url:    <img src=b.cover_url> with style "--accent: {b.accent || default}"
     └─ else:              stylized CSS template using metadata + b.accent
```

**Shadow paths**: nil cover → `None`, accent_color NULL, frontend uses theme default; corrupt bytes → `image::load_from_memory` Err → `None`; all-black or grayscale → no chromatic content → `None`. Indexer never halts on a single cover.

### 2c. Atrium CSS load (web)

The stylesheet is referenced through Dioxus's [`asset!`](https://docs.rs/dioxus/latest/dioxus/macro.asset.html) macro, which hashes the file at build time and routes the request through the Dioxus dev/prod asset pipeline (no hand-written `ServeDir` mount required). The macro returns an `Asset` value that `App()` plugs into a `document::Stylesheet { href: ATRIUM_CSS }` element next to the existing legacy `<style>` block.

```
asset!("/assets/atrium.css")                  // at compile time
  → frontend/assets/atrium.css                // resolved relative to the crate root
  → hashed asset path emitted into the bundle
  → Dioxus runtime serves the bytes with long-lived cache headers

Browser ──GET /assets/atrium.<hash>.css──▶ Dioxus asset handler
Browser ──GET https://fonts.googleapis.com/…──▶ Google Fonts CDN (foundation PR)
```

Fonts are pulled from Google Fonts in the foundation PR via the `@import` at the top of `atrium.css`. Self-hosting (`@font-face` under `frontend/assets/fonts/`) is the follow-up that unblocks offline / airgapped installs. The choice is intentional and lives in the file header.

**Shadow paths**: missing CSS → asset pipeline build error blocks the binary; runtime 404 → page renders with the legacy `STYLES` constant still in `lib.rs` (no FOUC fallback yet — added with self-hosted fonts); blocked Google Fonts request (CSP / offline) → `font-display: swap` keeps text readable with the system fallback.

---

## 3. Storage shape

### Migration `db/migrations/0006_books_accent.sql`

```sql
ALTER TABLE books ADD COLUMN accent_color TEXT;
CREATE INDEX IF NOT EXISTS idx_books_accent_null
  ON books(id) WHERE accent_color IS NULL;
```

The partial index lets a future "backfill missing accents" worker job find unprocessed rows cheaply.

**Reverse** is a manual `ALTER TABLE books DROP COLUMN accent_color` (sqlx migrate-down isn't used here). SQLite ≥ 3.35 supports it.

### Column

| Column | Type | Notes |
|---|---|---|
| `accent_color` | `TEXT` | Opaque CSS color value. Today we write `oklch(L C H)` (e.g. `"oklch(0.66 0.13 245)"`); the column is format-agnostic so we can move to hex or named tokens later without a schema change. NULL → frontend uses theme default. |

### Hottest queries

1. `SELECT id, title, ..., accent_color FROM books WHERE library_id = ?` — landing list.
2. `SELECT ..., accent_color FROM books WHERE id = ?` — book detail.
3. (future) `SELECT id FROM books WHERE accent_color IS NULL LIMIT 100` — backfill worker; partial index covers this.

No new tables. No `COLLATE NOCASE` — accent is a programmatic value, not searchable.

### Shared type change (`shared/src/lib.rs`)

```rust
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct EbookMetadata {
    // existing fields…
    pub accent: Option<String>, // CSS color, null = use theme default
}
```

---

## 4. Failure modes

| Failure | Cause | Detection | Recovery | User sees |
|---|---|---|---|---|
| `AccentDecodeFailed` | cover bytes corrupt / unsupported format | `image::load_from_memory` returns Err | inline `None` return | Default theme accent |
| `AccentNoChroma` | all-black or pure-grayscale cover | Hue-bucket pass finds zero weight | inline `None` return | Default accent |
| `MigrationFailed` | sqlx column add fails | `sqlx::migrate!` returns Err on boot | Server fails to start; rollback previous binary | 503 |
| `ThemeStorageWriteFailed` | localStorage quota / private mode / mobile r/o fs | `Result::Err` from set call | In-memory only for session; log warn | Toggle works but doesn't persist |
| `AtriumCssMissing` | static file removed | Playwright smoke asserts computed font-family includes "Geist" | CI fails; deploy blocked | (would be) unstyled |
| `FontLoadBlocked` | CSP blocks self-hosted font | DevTools console; `font.load()` rejects | `font-display: swap` → system fallback renders | Mild FOUT |

---

## 5. Rollback plan

- **Accent extraction**: migration is forward-compatible (nullable column). Roll back by deploying previous binary — column persists, no data loss. If extraction misbehaves at runtime, set `OMNIBUS_EXTRACT_ACCENT=0` env var; indexer skips the step.
- **Atrium CSS**: rollback = revert the foundation PR. The Library reskin lives in a separate PR; reverting it restores the previous landing while keeping the design system available for follow-up work.
- **Theme toggle**: cleanly removable; deleting the toggle component reverts to dark-only. Stored values are harmless leftovers.

All changes in this delivery are reversible.

---

## 6. Observability

### Logs (`tracing`)

- `db::ebook::extract_accent`: silent on success path (called once per book during reindex — too noisy at debug level for large libraries). Errors are reflected by NULL `accent_color` rows, which an admin can grep on.
- Indexer summary at end of every reindex: `info!("indexer summary: books={n} accents_extracted={a} accents_failed={f}")` (added in a follow-up; not blocking for foundation).

### Metrics / alerts / dashboards

N/A — no metrics pipeline yet. Tracked under [F5.2 Observability](../roadmap/5-2-observability.md).

---

## 7. Open questions

### Resolved

- *Scope* → Foundation first; no page reskin in this PR. Library reskin (F1.7-b) is the first page port and lands separately.
- *Covers* → Real cover image + server-side extracted accent. Stylized templates fallback only.
- *CSS arch* → Static `frontend/assets/atrium.css` referenced via Dioxus `asset!`. Hashing + serving is handled by the asset pipeline; no hand-written axum static mount.
- *Theme tweaks* → Dark + light only; sepia/density/type deferred to [F1.9](../roadmap/1-9-themes-and-density.md).
- *Theme attribute location* → on the `.atrium` wrapper div emitted by `AtriumRoot`, not on `<html>`. Keeps the swap declarative (Dioxus re-render) instead of imperative (DOM mutation from Rust).
- *Cover-color algorithm* → Hue-bucket histogram on a 32×48 downsample; pick highest-weighted bucket; OKLab matrix to OKLCH; clamp lightness into a readable band.
- *Dev port* → 3001 during the parallel-agent phase so we don't collide with the default workspace's 3000.

### Unresolved

- *Self-hosted fonts* — Foundation PR uses Google Fonts CDN. Self-hosting `@font-face` files under `frontend/assets/fonts/` is a follow-up inside F1.7 (small, but needs the woff2 binaries and a license check).
- *`EbookMetadata.accent` typing* — keep `Option<String>` for v1. Type later if we need schema validation.
- *Top-nav routes for un-built sections* (Reading / Listen / Journal / You) — render as disabled `aria-disabled` chips until each route exists.

---

## 8. Test plan

### Rust unit tests

- `db::ebook::tests`:
  - `extract_accent_returns_oklch_for_saturated_cover` (happy path, in-memory PNG)
  - `extract_accent_returns_none_for_empty_bytes`
  - `extract_accent_returns_none_for_corrupt_bytes`
  - `extract_accent_returns_none_for_pure_black`
  - `extract_accent_returns_none_for_pure_gray`
  - `extract_accent_completes_within_budget` (1500×2250 worst case ≤ 500 ms debug)
- `db::queries::tests`:
  - existing `replace_books_inserts_metadata_and_covers` extends to assert `accent` round-trips.

### Integration

- `server::backend::tests::ebooks_endpoint_returns_accent_field` (wire format smoke).

### Playwright E2E

- Update `tests/flows/landing.spec.ts`: grid items expose `--accent` via a `data-testid="cover"` element.
- New `tests/flows/theme-toggle.spec.ts`:
  - Layout: cold load → `<html data-theme="dark">`, `--bg-0` matches dark token.
  - Action: toggle → `data-theme="light"`, `localStorage["omn.theme"] === "light"`; reload preserves; error path with stubbed `localStorage.setItem` stays session-only and reloads back to dark.

### Not tested (and why)

- Visual diff of every cover template — Playwright pixel-diffs are flaky on font subpixel rendering. Tracked under [F5.2](../roadmap/5-2-observability.md) for visual regression tooling.
- Per-book contrast WCAG validation. We clamp lightness to a readable band; full validation would gate indexing on a slow check. Future: admin-surface contrast warning.

---

## 9. Roadmap doc additions

| New file | Tracks | Status |
|---|---|---|
| [F1.7 Atrium design system](../roadmap/1-7-atrium-design-system.md) | This delivery — tokens, primitives, Library reskin | In progress |
| [F1.8 Discovery pages](../roadmap/1-8-discovery-pages.md) | Author / Series / Tag-cloud pages | Planned |
| [F1.9 Themes & density](../roadmap/1-9-themes-and-density.md) | Sepia theme, density toggle, type pairing toggle, user-preferences table | Planned |
| [F3.4 Stats](../roadmap/3-4-stats.md) | Year-in-review reading stats | Planned (depends on F2.1) |
| [F3.5 Shared shelves](../roadmap/3-5-shared-shelves.md) | Multi-user shelves | Planned (depends on F3.1) |
| [F5.6 Admin health](../roadmap/5-6-admin-health.md) | Server health dashboard | Planned (splits from F5.4) |
| [F5.7 Journal & quote cards](../roadmap/5-7-journal-quote-cards.md) | Markdown journal + quote card composer | Planned (extends F3.2) |

Existing initiatives that gain an Atrium "redesign" sub-task:

- F1.1 Search · F1.4 Book detail · F0.3 Login · F2.2 Reader · F2.3 Player · F5.1 Metadata edit · F6.1 Mobile.

---

## 10. Implementation sequencing

1. **F1.7-a Foundation.** Migration + `extract_accent` + atrium.css + fonts + primitives + theme toggle. *No page changes yet.* Reviewable as infra.
2. **F1.7-b Library reskin.** Port `landing.rs` to Atrium primitives. Updated Playwright.
3. Follow-ups (in roadmap priority order): F1.4-redesign, F1.1-redesign, F0.3-login-redesign, F1.8, F1.9, then everything else picks up the primitives for free.

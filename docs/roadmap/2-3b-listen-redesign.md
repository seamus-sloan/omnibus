# F2.3b — Listen page redesign

**Phase 2 · Reading & listening** · **Priority:** P1

Companion to [F2.3](2-3-audiobook-player.md). Revamps the audiobook player UI
to match the Atrium design system and adds chapter navigation, bookmarks,
sleep timer, and a persistent mini-dock player.

## Objective

Transform the listen page from a basic two-column player into a cinematic,
feature-rich audiobook experience with chapter-aware navigation, playback
bookmarks, a sleep timer, and a persistent mini-dock bar for cross-page
listening.

## Status

- **PR 1 — Visual redesign + speed panel:** shipped.
- **PR 2 — Chapter infrastructure:** shipped.
- **PR 3 — Chapter UI:** shipped.
- **PR 6 — Mini-dock player:** shipped. Playback state and the `<audio>`
  element moved to the App root (`PlaybackState` context + App-level driver),
  so a persistent bottom dock keeps playing across pages. The dock renders in
  the web `ScreenLayout` (absent on the immersive `/listen` + `/read` routes),
  with cover/title/chapter/progress, ±30s + play/pause, speed, Expand, and a
  dismiss button.
- **PR 4 — Bookmarks:** shipped. Built on the unified bookmarks backend
  (shared `Bookmark` type, `db::bookmarks`, `/api/bookmarks*` REST +
  `/api/rpc/bookmarks/*`). The drawer lists/creates/notes/deletes marks and a
  "Bookmark saved" toast confirms a save.
- **PR 5 — Sleep timer:** shipped. Self-re-arming countdown (presets +
  end-of-chapter), live `Sleep · MM:SS` toolbar label, and pause + optional
  30s volume fade on expiry (`OmnibusAudio.setVolume`). Session-only.

## PR breakdown

### PR 1 — Visual redesign + speed panel

Pure frontend. No backend or DB changes.

- Migrate all listen-page inline styles to `.lp-*` CSS classes in `atrium.css`
- Accent-derived radial gradient backdrop (uses existing `EbookMetadata.accent`)
- Decorative border ring around the cover
- "Now playing" kicker label
- Restyled transport row: chapter-skip placeholders (disabled), ±30s, play/pause
- Speed panel overlay: preset grid (0.5–2.0×), fine-tune slider (0.5–3.0×),
  ±0.05 stepper. Frosted glass with scrim
- Toolbar row: Sleep, Bookmark, Chapters buttons (inert until their PRs land)
- Per-book playback speed persistence (`omn.listening.rate::{uuid}`)
- Custom-styled scrubber with accent-coloured fill + thumb
- Responsive: single-column on narrow viewports

### PR 2 — Chapter infrastructure

Backend + DB. Creates the chapter data pipeline.

- **Migration:** `file_chapters(id, book_file_id, ordinal, title, start_seconds,
  duration_seconds)` with a unique constraint on `(book_file_id, ordinal)`
- **Extraction:** parse chapter atoms from m4b containers via `lofty` (MP4 chapter
  atom list) and ID3 chapter frames from mp3 (CHAP/CTOC). Run during
  `reindex_audiobooks` in the indexer
- **Query:** `get_chapters(pool, book_file_id) -> Vec<Chapter>` in `db/src/queries.rs`
- **API:** extend `AudiobookManifest::Direct` to include
  `chapters: Vec<ChapterInfo>` in the manifest endpoint response
- **Shared type:** `ChapterInfo { ordinal, title, start_seconds, duration_seconds }`
  in `shared/src/lib.rs`
- **Fallback:** books with no extractable chapters get a single synthetic chapter
  spanning the full duration, so the frontend can always assume `chapters.len() >= 1`

### PR 3 — Chapter UI

Frontend chapter features. Depends on PRs 1 + 2.

- **Chapter map component:** duration-weighted bar chart replacing the plain
  scrubber. Each chapter is a flex container sized by `flex: {duration}`,
  containing vertical bars. Played = `var(--accent)`, upcoming = grey. Current
  chapter highlighted with an accent band and "now playing" label
- **Chapter skip buttons:** wire `◀ CH` / `CH ▶` to seek to previous/next
  chapter start. Uses the chapter `start_seconds` from the manifest
- **Chapters drawer:** right-side full-height panel listing all chapters. Played
  chapters show ✓, current shows animated equaliser bars, upcoming show padded
  number. Current chapter row highlighted with progress info
- **Kicker update:** "Now playing · Chapter X of Y" with live tracking
- **Chapter subtitle:** "Ch. 14 · A Sea of Glass and Fire" below the author
- **New signals:** `chapters: Signal<Vec<ChapterInfo>>`,
  `current_chapter_index: Signal<usize>`, computed from `elapsed` vs
  chapter `start_seconds` boundaries

### PR 4 — Bookmarks

Depends on PR 1. Uses the existing `bookmarks` table from migration 0013.

- **API endpoints:**
  - `POST /api/bookmarks` — create bookmark at current position
  - `GET /api/bookmarks/{book_uuid}` — list bookmarks for a book
  - `DELETE /api/bookmarks/{id}` — remove a bookmark
  - `PUT /api/bookmarks/{id}` — update title/note
- **Bookmarks drawer:** right-side panel mirroring the chapters drawer layout.
  Each row shows a flag icon, chapter name, optional note, timestamp. Freshly
  added bookmark highlighted with accent ring
- **Confirmation toast:** pill-shaped toast at top: "Bookmark saved" with
  timestamp and chapter number, auto-dismisses after 3 seconds
- **Note editing:** "Add a note…" placeholder on fresh bookmarks; clicking
  opens inline text input
- **Wire toolbar button:** Bookmark button in the toolbar row toggles the drawer

### PR 5 — Sleep timer

Pure frontend. Depends on PR 1.

- **Sleep panel:** frosted-glass overlay (same `.lp-panel` base as speed panel).
  Preset grid: Off, 15 min, 30 min, 45 min, 1 hour, 2 hours, 3 hours, 4 hours.
  Plus an "End of chapter N" option (needs chapter data from PR 3 when available,
  gracefully degrades to time-only when chapters aren't loaded)
- **Client-side countdown:** JS `setTimeout`-based timer. Toolbar Sleep button
  label shows live countdown ("Sleep · 28:42") when active
- **On expiry:** pause audio playback. Optionally fade volume over last 30 seconds
  (controlled by a toggle in the panel, default on)
- **Persistence:** sleep timer is session-only (not persisted across page loads)

### PR 6 — Mini-dock player

Depends on PRs 1–5. Significant architecture change.

- **Persistent bar:** bottom-docked bar visible on library, book detail, author,
  series, and other pages while an audiobook is playing
- **Architecture:** playback signals (`book`, `elapsed`, `duration`, `playing`,
  `rate`) move from route-local in `listen.rs` to app-global via
  `use_context_provider` at the App level. The listen page reads from context
  instead of creating its own signals
- **Layout:** cover thumbnail (44px), title + chapter, mini progress bar,
  compact transport (±30s, play/pause), speed display, sleep label, expand button
- **Expand button:** navigates to `/listen/:uuid` (full player)
- **Collapse:** navigating away from `/listen/:uuid` auto-shrinks to the dock
- **Audio element:** the hidden `<audio id="omnibus-audio">` element moves to
  the App root so it persists across route changes

## Dependencies

- [F2.3 Audiobook player](2-3-audiobook-player.md) — foundation (merged)
- [F2.1 Progress sync](2-1-progress-sync.md) — position persistence (merged)
- [F0.1 Schema refactor](0-1-schema-refactor.md) — `book_files` table (merged)

---

[← Back to roadmap summary](0-0-summary.md)

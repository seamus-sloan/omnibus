# Omnibus Roadmap (v2)

Supersedes the prior root-level `ROADMAP.md`. Synthesizes the v1 feature list with the [Calibre-Web inspection](../calibre-inspection/0-overview.md), reorganized to reflect the foundational work v1 assumed but did not sequence, and to incorporate performance lessons learned from Calibre-Web.

---

## 1. Vision

Omnibus is a self-hosted book library for personal use — the ebook and audiobook equivalent of Plex/Jellyfin. It scans local directories of ebooks (epub) and audiobooks (m4a/m4b), organizes them into user-configurable libraries filtered by metadata, and lets authenticated users browse, read, listen, journal, and rate books. It aims to go beyond what Calibre-Web and AudioBookShelf offer, with first-class device sync (Kobo, Kindle), in-browser reading and listening, cross-device progress tracking, and a native mobile app built from the same Rust codebase.

---

## 2. Executive summary

The v1 roadmap was a strong **feature wishlist** but a weak **execution plan**. It sequenced 14 user-visible features without calling out the schema, auth, and search foundations almost every feature depends on. It also predated the repo's current implementation — several "to-do" steps (books table, cover handling, scanner) already exist in a shape that conflicts with what v1 described.

The [Calibre-Web inspection](../calibre-inspection/0-overview.md) reveals three classes of issue we can avoid if we act early:

- **Schema foundation.** Calibre-Web's performance PRs were retrofits against a denormalized schema. We are about to make the same mistake: our current `books` table is one row per file with JSON-blob relationships. Fixing this once, early, is cheaper than any later N+1 optimization.
- **Search.** Calibre-Web shipped SQLite FTS5 as an opt-in fast path that only activates if Calibre-desktop created the index. We can make it the default.
- **Sequencing.** Calibre-Web's single `WorkerThread` is a performance ceiling it cannot lift without a rewrite. We have the stack to parallelize from day one — but only if we build the worker primitive before the first async feature needs it.

**Top-line recommendation:** insert a **Phase 0: Foundations** before any v1 feature ships, and reorder the remaining features around auth-first, search-early, and sync-last.

---

## 3. Current state assessment

### 3.1 What v1 got right

- Vision is crisp: "Plex/Jellyfin for books, beating Calibre-Web."
- Feature list is comprehensive and well-specified per feature (acceptance criteria, implementation plan).
- Stack choices (axum, sqlx, Dioxus fullstack + native) are correct and synergistic — see [Dioxus/Rust wins](../calibre-inspection/4-dioxus-rust-wins.md).
- Kindle delivery correctly uses epub directly (Amazon accepts it natively since 2022; no `ebook-convert` dependency).

### 3.2 Gaps

| # | Gap | Impact |
|---|---|---|
| G1 | **No search feature.** Browse-by-filter ≠ search. Calibre-Web users constantly hit this. | Core UX miss; users can't find a book they remember by title fragment. |
| G2 | **No book/book_files split.** One work with epub + m4b + pdf cannot be modeled. | Blocks multi-format; blocks Kindle-sends-epub-when-user-reads-m4b flow. |
| G3 | **No normalized authors/series/tags.** Current JSON blobs can't be filtered or FK'd. | Blocks Libraries, efficient browse, and reliable "books by X" queries. |
| G4 | **No migration framework.** Schema evolves by rewriting `initialize_schema`. | Blocks every shipping change after v1.0; risks data loss. |
| G5 | **No thumbnail pipeline.** v1 says "store cover path"; current code stores BLOBs. | Covers are the single most-rendered asset; performance lives or dies here. |
| G6 | **Auth is feature #7.** Six features come before it, implying single-user prototypes. | Every feature touching user data (ratings, progress, libraries) must be reworked post-auth. |
| G7 | **No metadata editing.** Calibre-Web's most-used admin surface is absent. | Users with messy libraries can't fix anything without hand-editing files. |
| G8 | **No observability.** No logging, metrics, or admin log viewer spec. | First production bug will be invisible. |
| G9 | **Kobo via OPDS only.** Calibre-Web's native Kobo-sync endpoint (`/kobo/v1/*`) is a closer UX than OPDS and isn't mentioned. | Leaves the best-supported Kobo integration off the table. |
| G10 | **No background worker abstraction.** Scanning, thumbnails, conversion, email all need async work, but there's no shared primitive planned. | Every feature reinvents its own task-queue or blocks the request path. |
| G11 | **No library filesystem convention.** Scanner walks anything, which is correct for read — but uploads have no target layout, so a UI-added book lands at the library root. Sidecar `cover.jpg` next to ebooks is ignored even though it's a common convention. | Uploaded libraries drift into chaos; slower cover rendering when a high-quality sidecar JPG already exists on disk. |

### 3.3 Ambiguities & contradictions

- **Scan paths plural in v1**, but current `Settings` has exactly two paths (`ebook_library_path`, `audiobook_library_path`). Which is correct?
- **Cover storage:** v1 says files on disk; current code stores BLOBs in `book_covers`. Need a decision before thumbnails ship.
- **Upload target:** v1 requires writing files into a scan path; but [calibre-inspection recommendations](../calibre-inspection/7-recommendations.md) argue against the DB mutating the filesystem for conflict-avoidance. These can coexist (uploads ≠ edits) but the distinction needs to be explicit.
- **Libraries are per-user** (v1 §3), but the OPDS feed (v1 §10) says "all libraries." Whose libraries does a Kobo see? Presumably the authenticating user's — clarify.

### 3.4 Outdated / already-changed assumptions

- v1 §1 "create `books` and `scan_paths` tables" — the `books` table exists but with a different shape.
- v1 §1 "store cover art as files in `covers/`" — current impl uses a BLOB table.
- v1 §14 "add `dioxus-mobile` crate target" — mobile crate already exists and ships.

---

## 4. Phasing

```
Phase 0  Foundations              (must-ship-before-everything)
Phase 1  Browse & Discovery       (the core library experience)
Phase 2  Reading & Listening      (primary user activity)
Phase 3  Personalization          (why users pick self-hosted)
Phase 4  Device Sync              (the Kobo/Kindle story)
Phase 5  Admin & Hygiene          (operating the server)
Phase 6  Mobile                   (feature parity on native)
```

| Phase | Theme | v1 features inside | New initiatives | Target horizon |
|---|---|---|---|---|
| 0 | Foundations | — | F0.1–F0.6 | short-term |
| 1 | Browse & discovery | #4 Views, #5 Detail | F1.1 Search, F1.2 Thumbnails | short-term |
| 2 | Reading & listening | #8 Reader, #9 Audio | F2.1 Progress sync service | medium-term |
| 3 | Personalization | #3 Libraries, #6 Ratings/journal, #12 Suggestions | — | medium-term |
| 4 | Device sync | #10 OPDS, #11 Kindle | F4.1 Native Kobo sync | medium-term |
| 5 | Admin & hygiene | #2 Uploads, #13 Admin | F5.1 Metadata edit, F5.2 Observability, F5.5 Conversion | long-term |
| 6 | Mobile | #14 Mobile | — | long-term |

---

## 5. Initiative index

### Phase 0 — Foundations

- [F0.1 Schema refactor](0-1-schema-refactor.md)
- [F0.2 Migration framework](0-2-migrations.md)
- [F0.3 Auth](0-3-auth.md)
- [F0.4 FTS5 index](0-4-fts5.md)
- [F0.5 Background worker primitive](0-5-background-worker.md)
- [F0.6 Library filesystem convention](0-6-library-filesystem.md)

### Phase 1 — Browse & discovery

- [F1.1 Search](1-1-search.md)
- [F1.2 Thumbnail pipeline](1-2-thumbnails.md)
- [F1.3 Library views](1-3-library-views.md)
- [F1.4 Book detail page](1-4-book-detail.md)

### Phase 2 — Reading & listening

- [F2.1 Progress sync service](2-1-progress-sync.md)
- [F2.2 In-browser epub reader](2-2-epub-reader.md)
- [F2.3 In-browser audiobook player](2-3-audiobook-player.md)

### Phase 3 — Personalization

- [F3.1 Libraries with metadata filters](3-1-libraries.md)
- [F3.2 Ratings & journaling](3-2-ratings-journaling.md)
- [F3.3 Suggestions](3-3-suggestions.md)

### Phase 4 — Device sync

- [F4.1 Native Kobo sync](4-1-kobo-sync.md)
- [F4.2 OPDS 1.2 feed](4-2-opds.md)
- [F4.3 Kindle delivery](4-3-kindle.md)

### Phase 5 — Admin & hygiene

- [F5.1 Metadata edit](5-1-metadata-edit.md)
- [F5.2 Observability](5-2-observability.md)
- [F5.3 Uploads](5-3-uploads.md)
- [F5.4 Admin panel](5-4-admin-panel.md)
- [F5.5 Format conversion](5-5-format-conversion.md)

### Phase 6 — Mobile

- [F6.1 Mobile app](6-1-mobile.md)

---

## 6. Changes relative to v1

### Cut (removed from v1.0 scope)

- **Hardcover integration** in suggestions. Ship local + OpenLibrary only; revisit on user demand.
- **Kindle auto-sync-per-library.** Manual "Send to Kindle" is P2; the rule-engine version is post-v1.0.
- **v1 §1 "files removed from disk flagged as missing".** Keep in scope but move to a hardening sub-task — the simple delete-on-rescan path ships first.

### Defer

- **Uploads** from Phase 1 → Phase 5. Not blocking self-hosters with existing sync tools.
- **In-app metadata editing from the detail page.** Lives in admin (F5.1) initially; per-book inline edit is a v1.1 polish.

### Merge

- **v1 #2 (Uploads) + #13 (Admin settings)** → Phase 5 admin panel with upload as a sub-action.
- **v1 #11 Kindle SMTP config + #13 admin SMTP settings** → one settings page, one SMTP config.

### Add

- **F0.1 Schema refactor** (was implicit; made explicit).
- **F0.2 Migrations** (gap G4).
- **F0.4 FTS5** (gap G1).
- **F0.5 Background worker** (gap G10).
- **F0.6 Library filesystem convention** (gap G11).
- **F1.1 Search** (gap G1).
- **F1.2 Thumbnail pipeline** (gap G5).
- **F2.1 Progress sync service** (unifies #8 and #9).
- **F4.1 Native Kobo sync** (gap G9).
- **F5.1 Metadata edit** (gap G7).
- **F5.2 Observability** (gap G8).
- **F5.5 Format conversion** (optional, deferred; shells out to `ebook-convert`).
- **kepubify integration** as a sub-scope of F4.1 (Kobo UX win).

### Reorder

- **Auth moves from #7 → Phase 0.** Cannot build features 3, 6, 8, 9, 11 correctly without it.
- **Kobo sync promoted above OPDS.** Better UX; see [calibre-inspection §6](../calibre-inspection/6-api-surface.md).

---

## 7. Tradeoffs, open questions, assumptions

### Tradeoffs

- **Phase 0 delays visible progress by 4–6 weeks.** Accepted — every week spent on Phase 0 saves multiple weeks of rework later.
- **Custom metadata via EAV/JSON, not per-column tables.** Trades some query flexibility for drastic schema simplicity vs. Calibre-Web's runtime-generated DDL.
- **FTS5 triggers add ~10% write amplification** on indexing. Acceptable; reads dominate.
- **Keyset pagination complicates "jump to page N" UX.** Calibre-Web has this UX but it's also the user-reported slow path. Swap for infinite-scroll + search.

### Open questions

1. **Cover storage:** BLOB (current) or filesystem cache? **Recommendation:** filesystem cache under `<data_dir>/thumbs/`. BLOBs bloat the DB and make backup heavier. Needs decision before [F1.2](1-2-thumbnails.md).
2. **Single scan path or multiple?** v1 says "one or more configured directories"; current code has two typed paths. **Recommendation:** array of typed paths `Vec<LibraryPath { path, kind }>`. Needed for [F0.1](0-1-schema-refactor.md).
3. **Mobile sync protocol:** re-use `/kobo/v1/*` or dedicated `/api/*`? **Recommendation:** dedicated `/api/*` is the primary surface; Kobo layer wraps it.
4. **Does OPF round-trip ever matter?** **Recommendation:** commit to DB-as-truth; ship an OPF export as a convenience only.
5. **Feature-flag system?** None today. **Recommendation:** defer until v1.1 unless a Phase 0–3 feature clearly needs gradual rollout.

### Assumptions

- **A1:** Omnibus targets single-instance self-hosted deployments. No horizontal scaling, no Postgres, no object store.
- **A2:** Epub + m4b cover the long tail; pdf / cbz / azw3 are out of scope for v1.0. Removing `pdf` from `scanner.rs` supported extensions is a Phase 0 cleanup task.
- **A3:** Users have ≤100k books. Beyond that, keyset pagination alone won't save us and we'll need materialized per-library counts.
- **A4:** Kindle still accepts `.epub` via email. Verified as of Q1 2026; if Amazon regresses, F4.3 grows a converter.
- **A5:** Dioxus fullstack SSR is stable enough for production. If hydration mismatches keep biting us, we may fall back to axum + vanilla + island hydration.
- **A6:** `kepubify` is stable and maintained. It has been since 2016; risk is low. If abandoned, Kobo falls back to plain EPUB with no functional loss.
- **A7:** **We do not write our own format converters in Rust.** Every serious attempt (Calibre, Pandoc) is a decades-long effort. F4.1's kepubify and F5.5's ebook-convert are shell-outs to existing tools; that stays true indefinitely.

---

## 8. Immediate next steps

1. Review and approve Phase 0 scope ([F0.1](0-1-schema-refactor.md) – [F0.6](0-6-library-filesystem.md)).
2. File GitHub issues for each F-number, porting acceptance criteria from the prior roadmap where the feature survives into v2.
3. Resolve open questions 1 (covers) and 2 (scan paths) before [F0.1](0-1-schema-refactor.md) kicks off — both block schema design.
4. Spike [F0.3](0-3-auth.md) session cookies on web + bearer tokens on mobile to verify the auth model is unified across surfaces.

---

## Related

- [Calibre-Web inspection](../calibre-inspection/0-overview.md) — source-level study of the tool we're replacing.

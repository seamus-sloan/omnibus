# Omnibus Roadmap v2

**Owner:** Seamus Sloan · **Audience:** engineering + product leadership · **Status:** proposal, pending review

Supersedes the prior `ROADMAP.md` (deleted alongside this document's introduction). Synthesizes the v1 feature list with [calibre-web-analysis.md](calibre-web-analysis.md), reorganized to reflect the foundational work v1 assumed but did not sequence, and to incorporate performance lessons learned from Calibre-Web.

---

## 1. Vision

Omnibus is a self-hosted book library for personal use — the ebook and audiobook equivalent of Plex/Jellyfin. It scans local directories of ebooks (epub) and audiobooks (m4a/m4b), organizes them into user-configurable libraries filtered by metadata, and lets authenticated users browse, read, listen, journal, and rate books. It aims to go beyond what Calibre-Web and AudioBookShelf offer, with first-class device sync (Kobo, Kindle), in-browser reading and listening, cross-device progress tracking, and a native mobile app built from the same Rust codebase.

---

## 2. Executive summary

The v1 roadmap is a strong **feature wishlist** but a weak **execution plan**. It sequences 14 user-visible features without calling out the schema, auth, and search foundations that almost every feature depends on. It also predates the repo's current implementation — several "to-do" steps (books table, cover handling, scanner) already exist in a shape that conflicts with what v1 describes.

Calibre-Web analysis reveals three classes of issue we can avoid if we act early:

- **Schema foundation.** Calibre-Web's performance PRs were retrofits against a denormalized schema. We are about to make the same mistake: our current `books` table is one row per file with JSON-blob relationships. Fixing this once, early, is cheaper than any later N+1 optimization.
- **Search.** Calibre-Web shipped SQLite FTS5 as an opt-in fast path that only activates if Calibre-desktop created the index. We can make it the default.
- **Sequencing.** Calibre-Web's single `WorkerThread` is a performance ceiling it cannot lift without a rewrite. We have the stack to parallelize from day one — but only if we build the worker primitive before the first async feature needs it.

**Top-line recommendation:** insert a **Phase 0: Foundations** before any v1 feature ships, and reorder the remaining features around auth-first, search-early, and sync-last.

---

## 3. Current state assessment

### 3.1 What v1 got right

- Vision is crisp: "Plex/Jellyfin for books, beating Calibre-Web."
- Feature list is comprehensive and well-specified per feature (acceptance criteria, implementation plan).
- Stack choices (axum, sqlx, Dioxus fullstack + native) are correct and synergistic — see calibre-web-analysis §4.
- Kindle delivery correctly uses epub directly (Amazon accepts it natively since 2024; no `ebook-convert` dependency).

### 3.2 Gaps

| # | Gap | Impact |
|---|---|---|
| G1 | **No search feature.** Browse-by-filter ≠ search. Calibre-Web users constantly hit this. | Core UX miss; users can't find a book they remember by title fragment. |
| G2 | **No book/book_files split.** One work with epub + m4b + pdf cannot be modeled. | Blocks multi-format; blocks Kindle-sends-epub-when-user-reads-m4b flow. |
| G3 | **No normalized authors/series/tags.** Current JSON blobs can't be filtered or FK'd. | Blocks Libraries (#3), efficient browse, and reliable "books by X" queries. |
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
- **Upload target:** v1 requires writing files into a scan path; but [calibre-web-analysis.md §7](calibre-web-analysis.md#7-recommendations-for-omnibus) argues against the DB mutating the filesystem for conflict-avoidance. These can coexist (uploads ≠ edits) but the distinction needs to be explicit.
- **Libraries are per-user** (v1 §3), but the OPDS feed (v1 §10) says "all libraries." Whose libraries does a Kobo see? Presumably the authenticating user's — clarify.

### 3.4 Outdated / already-changed assumptions

- v1 §1 "create `books` and `scan_paths` tables" — the `books` table exists but with a different shape.
- v1 §1 "store cover art as files in `covers/`" — current impl uses a BLOB table.
- v1 §14 "add `dioxus-mobile` crate target" — mobile crate already exists and ships.

---

## 4. Revised roadmap

### 4.1 Phasing

```
Phase 0  Foundations              (must-ship-before-everything)
Phase 1  Browse & Discovery       (the core library experience)
Phase 2  Reading & Listening      (primary user activity)
Phase 3  Personalization          (why users pick self-hosted)
Phase 4  Device Sync              (the Kobo/Kindle story)
Phase 5  Admin & Hygiene          (operating the server)
Phase 6  Mobile                   (feature parity on native)
```

### 4.2 At-a-glance

| Phase | Theme | v1 features inside | New initiatives | Target horizon |
|---|---|---|---|---|
| 0 | Foundations | — | F0.1–F0.6 | short-term |
| 1 | Browse & discovery | #4 Views, #5 Detail | F1.1 Search, F1.2 Thumbnails | short-term |
| 2 | Reading & listening | #8 Reader, #9 Audio | F2.1 Progress sync service | medium-term |
| 3 | Personalization | #3 Libraries, #6 Ratings/journal, #12 Suggestions | — | medium-term |
| 4 | Device sync | #10 OPDS, #11 Kindle | F4.1 Native Kobo sync | medium-term |
| 5 | Admin & hygiene | #2 Uploads, #13 Admin | F5.1 Metadata edit, F5.2 Observability | long-term |
| 6 | Mobile | #14 Mobile | — | long-term |

---

## 5. Phase-by-phase detail

### Phase 0 — Foundations

Non-negotiable pre-work. Nothing in Phases 1–6 is safe to ship until these are in place. Target: 4–6 weeks.

#### F0.1 Schema refactor (books / book_files / normalized relations)

- **Objective.** Split `books` (logical work) from `book_files` (one row per format). Normalize `authors`, `series`, `tags`, `publishers`, `languages` into tables with m2m link tables.
- **Value.** Unblocks Libraries (#3), Search (F1.1), browse-by-author, and multi-format delivery (read epub on web, listen to m4b in car, send epub to Kindle — same work).
- **Tech.** Mirror Calibre's `data` table layout for path compatibility (see calibre-web-analysis §5). Add indices on every m2m reverse column and on `books.uuid`, `books.last_modified`, `books.sort`, `books.series_index`. `COLLATE NOCASE` on searchable strings.
- **Dependencies.** F0.2 (migrations) must land first or concurrently.
- **Risks.** Touches every read and write path currently in the repo. Needs a re-index after deploy — acceptable since the DB is already a rebuildable cache.
- **Priority.** P0.

#### F0.2 Migration framework

- **Objective.** Replace inline `initialize_schema` with versioned migrations.
- **Value.** Lets every subsequent change ship without risking production DBs.
- **Tech.** `sqlx::migrate!` or `refinery`. Inline schema stays for test DBs but migrations become source of truth.
- **Dependencies.** None.
- **Risks.** Low. One-time tooling cost.
- **Priority.** P0.

#### F0.3 Auth (moved up from v1 #7)

- **Objective.** Multi-user authentication with sessions; first-user-admin.
- **Value.** Every feature touching user state (progress, ratings, libraries, Kindle address) needs it. Building those first and retrofitting auth is strictly more work.
- **Tech.** `argon2` + `tower-sessions` backed by SQLite. `auth_required` / `admin_required` axum extractors. Explicit permission columns — **do not** copy Calibre-Web's role bitmask (see analysis §7 rec #9).
- **Dependencies.** F0.1 (users + FK'd tables).
- **Risks.** Session semantics in Dioxus fullstack + mobile need care — cookies on web, bearer tokens on mobile. Prototype both before committing.
- **Priority.** P0.
- **Cut from v1:** the "first registered user is admin" rule stays, but add a `OMNIBUS_INITIAL_ADMIN` env-var escape hatch for ops recovery.

#### F0.4 FTS5 index + trigger-based sync

- **Objective.** Create `books_fts` virtual table at startup with AFTER INSERT/UPDATE/DELETE triggers on `books` + joined relations.
- **Value.** bm25-ranked, tokenized, diacritic-insensitive search. No `LIKE '%q%'` anywhere.
- **Tech.** `unicode61 remove_diacritics 2` tokenizer. External-content table pattern so we don't duplicate storage. Re-populate on migration.
- **Dependencies.** F0.1, F0.2.
- **Risks.** Low; sqlite FTS5 is mature. Index rebuild on schema change is O(n) books.
- **Priority.** P0.

#### F0.6 Library filesystem convention

- **Objective.** Define, document, and enforce the on-disk layout Omnibus writes. Keep the read path permissive so users can point Omnibus at any existing folder (including a Calibre library) without reorganizing first — but **do not promise Calibre round-trip compatibility.** See "Calibre interop decision" below.

  Omnibus canonical layout:

  ```
  <library_root>/
    <author-slug>/
      <title-slug>/
        <title-slug>.epub          # one or more format files
        <title-slug>.m4b
        cover.jpg                  # optional sidecar; preferred over embedded if present
  ```

  No `metadata.opf` sidecar. No `(id)` suffix on folder names. Display-name authors, not `Lastname, Firstname`. Metadata lives in the DB; the filesystem stays human-readable and tool-agnostic.

- **Value.** (1) Uploads (F5.3) produce a predictable, human-browsable layout instead of dumping files at the library root. (2) If a user stops using Omnibus, the folder still makes sense in Finder, Syncthing, or a future self-hosted tool. (3) Removing OPF/Calibre-compat ambition eliminates a large class of edge cases before they ship.

- **Scope.**
  - **Read path — fully layout-tolerant.** Scanner walks arbitrary trees recursively (current behavior preserved). Metadata comes from each ebook's internal OPF (we already parse this). A Calibre library dropped in *works* via the normal scan path — we just don't advertise it as a first-class migration.
  - **Cover sidecar, opportunistic.** If `cover.jpg` / `cover.png` sits next to an ebook, prefer it over the embedded cover — faster (no epub re-open) and typically higher quality. This is a filename convention, not an OPF thing; zero interop cost.
  - **Ignore `metadata.opf` sidecars.** Every field they contain is also in the epub's internal OPF. The only deltas are Calibre-specific edits a user made in Calibre desktop — if they want those edits preserved, they should stay on Calibre. Parsing the sidecar would add a reconcile-with-embedded codepath we don't need.
  - **Write path — canonical layout, slugged filenames.** Uploads (F5.3) compute `<library_root>/<author-slug>/<title-slug>/<title-slug>.<ext>` from the metadata extracted from the uploaded file. Collision → suffix ` (2)` on the title folder. Never overwrite.
  - **No renaming on metadata edit.** Metadata overrides live in the DB (`books.metadata_overrides` JSON), never mutate folder names or file contents. Explicitly rejects Calibre-Web's folder-rename-on-edit path (racy with readers and scanners).

- **Calibre interop decision.** We are *not* a drop-in replacement for Calibre's library format. Reasons:
  1. Faithful Calibre layout requires the `(id)` suffix on title folders, `Lastname, Firstname` author folders via `opf:file-as`, Calibre's sanitization quirks (`:` → `_`), and `calibre:title_sort` / `calibre:series` / custom-column conventions. Copying all of that inherits 2006 desktop-era design decisions.
  2. The sidecar `metadata.opf` is bit-for-bit the OPF already inside the epub, plus two `calibre:*` meta tags. It's not a portable metadata standard — it's Calibre's state file. Treating it as authoritative input means reconciling two sources of truth for every book.
  3. Scanning an existing Calibre library already works today via the embedded-epub-OPF path. That's a nice-to-have, not a feature we should stake a roadmap claim on.
  4. If cross-tool migration becomes a demand signal post-v1.0, ship a dedicated one-shot importer (`omnibus import-calibre <path>`) that reads Calibre's `metadata.db` directly — cleaner than making the live scanner shaped like Calibre.

- **Tech.** A new `library_layout` module under `frontend/src/` with `canonical_path(metadata) -> PathBuf` and `sidecar_cover_for(ebook_path) -> Option<PathBuf>`. Unit tests against fixture trees that include (a) canonical Omnibus layout, (b) flat dump, (c) a minimal Calibre-shaped tree to confirm tolerant scan.
- **Slug rules.** ASCII-fold, lowercase, non-alphanumerics → `-`, collapse runs, trim to 80 chars. Document this; users will `ls` these folders. Keep the display name (with punctuation, case, unicode) in the DB — the slug is purely a filesystem artifact.
- **Dependencies.** F0.1 (need `authors` + `title` columns to compute slugs).
- **Risks.**
  - Users expect Calibre interop and are disappointed. Mitigation: explicit in docs, and the tolerant scan means their library still works — just not as "their Calibre library."
  - Slug collisions across unicode-equivalent titles. Mitigated by collision-suffix on write.
- **Priority.** P0. Blocks F5.3 uploads.
- **Open questions.**
  - Do we ship an optional "reorganize library into canonical layout" admin action? **Recommendation:** no for v1.0 — destructive, no user demand yet.
  - Do we want an OPF *export* action (write a fresh OPF when a user downloads a book) as a courtesy to users leaving Omnibus? **Recommendation:** defer. Zero cost to skip; easy to add later.

#### F0.5 Background worker primitive

- **Objective.** A single `tokio::task::JoinSet` + `Semaphore` + typed task enum driving scans, thumbnails, email, (future) conversion.
- **Value.** Avoids Calibre-Web's single-`WorkerThread` ceiling. Keeps web path responsive while CPU-bound work runs.
- **Tech.** In-memory queue initially (acceptable — we own the process). Persist to `background_tasks` table when admin log viewer (F5.2) arrives.
- **Dependencies.** None.
- **Risks.** Designing the task enum conservatively — additions are easy, renames are painful.
- **Priority.** P0.

### Phase 1 — Browse & discovery

The first Omnibus experience a user sees. Target: 3–4 weeks after Phase 0.

#### F1.1 Search (new — gap G1)

- **Objective.** Single search box on every page; queries FTS5; returns ranked results; supports `author:`/`series:`/`tag:` facets.
- **Value.** The feature Calibre-Web users most complain about. Cheap to ship on top of F0.4.
- **Tech.** `SELECT … FROM books_fts WHERE books_fts MATCH ? ORDER BY bm25(books_fts)`. Dioxus signal-debounced input.
- **Dependencies.** F0.4.
- **Risks.** None material.
- **Priority.** P0 within Phase 1.

#### F1.2 Thumbnail pipeline (new — gap G5)

- **Objective.** On-demand cover resizing with WebP cache on disk at 3 sizes; `srcset` responsive delivery.
- **Value.** Cover grids are the highest-bandwidth path. WebP at 3 sizes delivers ~30% smaller payloads than Calibre-Web's scheduled JPEG pipeline.
- **Tech.** `image` crate + `webp` crate. Cache at `<data_dir>/thumbs/<book_id>_<size>.webp`. Invalidate on `book.last_modified` bump. Generated by F0.5 worker.
- **Dependencies.** F0.5, cover-storage decision (ambiguity 2.3).
- **Risks.** Disk footprint on 100k-book libraries. Mitigation: LRU eviction past a configurable cap.
- **Priority.** P0.

#### F1.3 Library views (v1 #4)

- **Objective.** Table view + cover grid; view preference persisted per library.
- **Changes from v1.** Pagination via keyset (not `OFFSET`) — calibre-web-analysis §3 shows `OFFSET` is the large-library killer. Sort happens client-side on already-hydrated lists for ≤10k-book libraries (leverage Dioxus signals; see analysis §7 rec #12).
- **Dependencies.** F0.1, F1.2.
- **Priority.** P1.

#### F1.4 Book detail page (v1 #5)

- **Objective.** Full metadata + read/listen CTA + breadcrumb.
- **Changes from v1.** Detail page is the host for Ratings (Phase 3) and Suggestions (Phase 3), as v1 specifies — but those slots ship empty in Phase 1 and are filled later.
- **Dependencies.** F0.1.
- **Priority.** P1.

### Phase 2 — Reading & listening

The primary user activity. Target: 4–6 weeks after Phase 1.

#### F2.1 Progress sync service (new)

- **Objective.** Single `POST /api/progress` endpoint taking a discriminated payload `{ epub_cfi }` or `{ audio_position_seconds }`.
- **Value.** Reader (#8) and audio player (#9) share a sync mechanism; mobile gets it for free. Avoids duplicating debounce/offline-queue logic per format.
- **Tech.** Unified `reading_progress` table with `format` discriminator column. Last-write-wins; reconcile conflicts client-side.
- **Dependencies.** F0.3.
- **Priority.** P0 within Phase 2.

#### F2.2 In-browser epub reader (v1 #8)

- **No scope changes** from v1. epub.js integration.
- **Dependencies.** F2.1.
- **Priority.** P1.

#### F2.3 In-browser audiobook player (v1 #9)

- **No scope changes** from v1. HTML5 audio + chapter table.
- **Dependencies.** F2.1, F0.1 (chapters belong on `book_files` or a new `file_chapters`).
- **Priority.** P1.

### Phase 3 — Personalization

Features that differentiate self-hosted Omnibus from a bookstore. Target: 4 weeks after Phase 2.

#### F3.1 Libraries with metadata filters (v1 #3)

- **Changes from v1.** Filter rules now operate on normalized columns from F0.1 instead of JSON blobs — significantly simpler. Admin-vs-user scoping stays.
- **Dependencies.** F0.1, F0.3.
- **Priority.** P1.

#### F3.2 Ratings & journaling (v1 #6)

- **No scope changes** from v1.
- **Dependencies.** F0.1, F0.3.
- **Priority.** P2.

#### F3.3 Suggestions (v1 #12)

- **Changes from v1.** Drop Hardcover for v1.0; local + OpenLibrary only. Hardcover adds per-user API key management, rate-limit handling, and caching complexity not justified until user demand appears.
- **Dependencies.** F0.1.
- **Priority.** P3.

### Phase 4 — Device sync

The "this is why I self-host" story. Target: 4 weeks after Phase 3.

#### F4.1 Native Kobo sync (new — gap G9)

- **Objective.** Implement `/kobo/v1/library/sync`, `/state`, `/metadata`, `/tags`, `download/*` endpoints (see calibre-web-analysis §6).
- **Value.** Far superior to OPDS on Kobo: background sync, reading-state round-trip, shelves-as-tags, no manual re-browse.
- **Tech.** Stream the sync response via `axum::body::StreamBody` — **do not** copy Calibre-Web's 100-item cap (analysis §3, §7 rec #13).
- **Includes: EPUB → KEPUB conversion via [kepubify](https://github.com/pgaskin/kepubify).** Kobo devices render plain EPUB but with measurably slower page turns than KEPUB; shipping the plain file is leaving UX on the table. Approach: detect kepubify on `PATH` at startup; on first Kobo download of each book, run `kepubify` via F0.5 worker, cache output at `<data_dir>/kepub/<book_id>.kepub.epub`, serve that on subsequent requests. Invalidate cache on `book.last_modified`. If kepubify is absent, fall back to the plain EPUB with a one-time admin warning in the log. Bundle kepubify in the Nix dev shell and in release images; keep it optional at runtime for users who build from source.
- **Dependencies.** F0.1 (uuid index), F0.3, F0.5, F2.1.
- **Risks.** Undocumented Kobo protocol; Calibre-Web's implementation is our reference. Scope to "parity with Calibre-Web minus 100-item cap" for v1.0.
- **Priority.** P1. Promote ahead of OPDS — it's the better UX for the platform that matters most.

#### F4.2 OPDS 1.2 feed (v1 #10)

- **Objective.** Serve OPDS Atom catalog for everything-that-isn't-Kobo (KOReader, Moon+ Reader, Marvin).
- **Changes from v1.** Added OPDS 2.0 (JSON) feed — trivial given the Kobo sync endpoint's JSON shape is adjacent.
- **Dependencies.** F0.3.
- **Priority.** P2.

#### F4.3 Kindle delivery (v1 #11)

- **Changes from v1.** Clarify: epub direct, no conversion. Defer auto-sync-on-new-book to post-v1.0 (the per-library deliverable is a complex rule engine; ship manual "send" first).
- **Dependencies.** F0.3, F0.5.
- **Priority.** P2.

### Phase 5 — Admin & hygiene

Operating the server. Target: 3 weeks after Phase 4.

#### F5.1 Metadata edit (new — gap G7)

- **Objective.** In-UI editing of title, authors, series, tags, description, cover replace.
- **Value.** Self-hosted libraries are messy; the ability to fix metadata without shelling into the server is table stakes. Absence of this is Calibre-Web's biggest retention hook.
- **Tech.** **Edits go to the DB only**, never to disk (analysis §7 rec #7, #8). `books.metadata_overrides` JSON column merged on read. OPF export exists only as a download action.
- **Dependencies.** F0.1, F0.3.
- **Risks.** Merge rules between scanned values and overrides need to be explicit and tested.
- **Priority.** P1 within Phase 5.

#### F5.2 Observability (new — gap G8)

- **Objective.** `tracing` structured logs, `/metrics` prometheus endpoint, admin log viewer, background-task dashboard.
- **Value.** First production bug is free of guesswork; scheduled-task visibility is a Calibre-Web feature we should not regress.
- **Tech.** `tracing` + `tracing-subscriber` → JSON to disk; `axum-prometheus` for HTTP metrics; `background_tasks` table feeding the admin view.
- **Dependencies.** F0.5.
- **Priority.** P1.

#### F5.3 Uploads (v1 #2)

- **Changes from v1.** Demoted from Phase 1 to Phase 5. Most self-hosters already ship books to the library via Syncthing/rsync/NFS; in-UI upload is convenience, not a blocker.
- **Dependencies.** F0.3, F0.5.
- **Priority.** P2.

#### F5.5 Format conversion (new, optional, deferred)

- **Objective.** Surface a "Convert to…" action on the book detail page (and in admin bulk actions) that shells out to Calibre's `ebook-convert` binary when present on the host. No bundled converter.
- **Value.** Gives power users the ability to generate AZW3, PDF, TXT, FB2, etc. without leaving Omnibus. Closes a real Calibre-Web parity gap for the subset of users who actually use it.
- **Scope cap.**
  - **In scope:** generic `ebook-convert` shell-out for any format pair Calibre supports; per-conversion async job via F0.5 worker; result stored as a new row in `book_files` (formats coexist for the same work — clean because of F0.1).
  - **In scope:** EPUB → KEPUB via kepubify is **already** shipped by F4.1; this initiative reuses that infrastructure.
  - **Out of scope for v1.x:** CBZ/CBR generation (different audience); audiobook transcoding (m4a ↔ mp3); any Rust-native converter (see assumption A7).
- **Tech.** Config flag `ebook_convert_path` (auto-detected on startup, overridable in admin). Task type `ConvertFormat { book_id, source_format, target_format }` posted to the F0.5 worker. Timeout generous — some conversions take minutes. Surface progress/completion through F5.2 observability.
- **Dependencies.** F0.1, F0.5, F5.2, F5.4 (admin UI for the config flag).
- **Risks.**
  - Conversion quality is frequently poor (PDF → EPUB is famously bad); users will blame Omnibus for Calibre's output. Mitigation: docs clearly label this as a pass-through to Calibre.
  - Resource consumption — `ebook-convert` is CPU- and memory-heavy. F0.5's semaphore must cap concurrent conversions (e.g., `max(1, num_cpus / 2)`).
  - Deployment complexity: Calibre is ~300 MB. Keep it optional; document the install path for Docker/Nix deployments.
- **Priority.** P3. **Post-v1.0 unless user demand appears.** Most users asking for format conversion already have Calibre installed; they can convert there.
- **Open question.** Should Omnibus ship a "convert on upload" mode (e.g. "always convert uploaded AZW3 to EPUB for browser reading")? **Recommendation:** no — keeping formats as the user uploaded them preserves fidelity; convert on demand instead.

#### F5.4 Admin panel (v1 #13)

- **Changes from v1.** Absorbs F5.1 (metadata edit) and F5.2 (observability) as sub-sections. Scan path management and SMTP config stay.
- **Dependencies.** F5.1, F5.2, F4.3.
- **Priority.** P2.

### Phase 6 — Mobile

Feature parity on native. Target: 6+ weeks after Phase 4 (needs device-sync endpoints).

#### F6.1 Mobile app (v1 #14)

- **Changes from v1.** Mobile crate already exists; the v1 "add `dioxus-mobile`" step is done. Focus shifts to: first-launch server-URL setup screen, offline download, background audio, system media controls, progress sync integration.
- **Dependencies.** F2.1, F4.1 (re-use sync protocol for offline queue).
- **Priority.** P2.
- **Open question.** Does Dioxus Native give us enough access to `AVAudioSession` / `MediaSession` without a platform bridge crate? Prototype required before committing scope.

---

## 6. Changes relative to v1

This section captures the delta from the prior `ROADMAP.md` for reviewers familiar with it. Once v2 is approved and the prior document is removed, it remains a useful record of design intent.

### Cut (removed from v1.0 scope)

- **Hardcover integration** in suggestions. Ship local + OpenLibrary only; revisit on user demand.
- **Kindle auto-sync-per-library**. Manual "Send to Kindle" is P2; the rule-engine version is post-v1.0.
- **v1 §1 "files removed from disk flagged as missing"**. Keep in scope but move to a hardening sub-task — the simple delete-on-rescan path ships first.

### Defer

- **Uploads** from Phase 1 → Phase 5. Not blocking self-hosters with existing sync tools.
- **In-app metadata editing from the detail page**. Lives in admin (F5.1) initially; per-book inline edit is a v1.1 polish.

### Merge

- **v1 #2 (Uploads) + #13 (Admin settings)** → Phase 5 admin panel with upload as a sub-action.
- **v1 #11 Kindle SMTP config + #13 admin SMTP settings** → one settings page, one SMTP config.

### Add

- **F0.1 Schema refactor** (was implicit; made explicit).
- **F0.2 Migrations** (gap G4).
- **F0.4 FTS5** (gap G1).
- **F0.5 Background worker** (gap G10).
- **F1.1 Search** (gap G1).
- **F1.2 Thumbnail pipeline** (gap G5).
- **F2.1 Progress sync service** (unifies #8 and #9).
- **F4.1 Native Kobo sync** (gap G9).
- **F5.1 Metadata edit** (gap G7).
- **F5.2 Observability** (gap G8).
- **F0.6 Library filesystem convention** (gap G11).
- **kepubify integration** as a sub-scope of F4.1 (Kobo UX win).
- **F5.5 Format conversion** (optional, deferred; shells out to `ebook-convert`).

### Reorder

- **Auth moves from #7 → Phase 0.** Cannot build features 3, 6, 8, 9, 11 correctly without it.
- **Kobo sync promoted above OPDS.** Better UX; analysis §6 shows it's the canonical integration.

---

## 7. Tradeoffs, open questions, assumptions

### Tradeoffs

- **Phase 0 delays visible progress by 4–6 weeks.** Accepted — every week spent on Phase 0 saves multiple weeks of rework later (see Calibre-Web PR #3476 as evidence of the cost of the alternative).
- **Custom metadata via EAV/JSON, not per-column tables.** Trades some query flexibility for drastic schema simplicity vs. Calibre-Web's runtime-generated DDL.
- **FTS5 triggers add ~10% write amplification** on indexing. Acceptable; reads dominate.
- **Keyset pagination complicates "jump to page N" UX.** Calibre-Web has this UX but it's also the user-reported slow path. Swap for infinite-scroll + search.

### Open questions

1. **Cover storage:** BLOB (current) or filesystem cache (v1 §1)? **Recommendation:** filesystem cache under `<data_dir>/thumbs/`. BLOBs bloat the DB and make backup heavier. Needs decision before F1.2.
2. **Single scan path or multiple?** v1 says "one or more configured directories"; current code has two typed paths (ebook, audiobook). **Recommendation:** array of typed paths `Vec<LibraryPath { path, kind }>`. Needed for F0.1.
3. **Mobile sync protocol:** re-use `/kobo/v1/*` or dedicated `/api/*`? **Recommendation:** dedicated `/api/*` is the primary surface; Kobo layer wraps it (analysis §7 rec #14). Already the repo's direction.
4. **Does OPF round-trip ever matter?** v1 is silent; analysis §7 rec #8 says no (read-only input, export-only output). **Recommendation:** commit to DB-as-truth; ship an OPF export as a convenience only.
5. **Feature-flag system?** None in v1 or repo today. **Recommendation:** defer until v1.1 unless a Phase-0–3 feature clearly needs gradual rollout.

### Assumptions

- A1: Omnibus targets single-instance self-hosted deployments. No horizontal scaling, no Postgres, no object store. If this changes, Phase 0 needs revisiting.
- A2: Epub + m4b cover the long tail; pdf / cbz / azw3 are out of scope for v1.0. Removing `pdf` from `scanner.rs` supported extensions is a Phase-0 cleanup task.
- A3: Users have ≤100k books. Beyond that, keyset pagination alone won't save us and we'll need materialized per-library counts.
- A4: Kindle still accepts `.epub` via email. Verified as of Q1 2026; if Amazon regresses, F4.3 grows a `kepubify`-style converter.
- A5: Dioxus fullstack SSR is stable enough for production. If hydration mismatches keep biting us (see memory on `web` feature-gating), we may fall back to axum + vanilla + island hydration.
- A6: `kepubify` is stable and maintained. It has been since 2016; risk is low. If abandoned, Kobo falls back to plain EPUB with no functional loss.
- A7: **We do not write our own format converters in Rust.** Every serious attempt (Calibre, Pandoc) is a decades-long effort. F4.1's kepubify and F5.5's ebook-convert are shell-outs to existing tools; that stays true indefinitely.

---

## 8. Immediate next steps

1. Review and approve Phase 0 scope (F0.1 – F0.6).
2. File GitHub issues for each F-number, porting acceptance criteria from the prior roadmap where the feature survives into v2.
3. Resolve open questions 1 (covers) and 2 (scan paths) before F0.1 kicks off — both block schema design.
4. Spike F0.3 session cookies on web + bearer tokens on mobile to verify the auth model is unified across surfaces.
5. Delete the prior `ROADMAP.md` once this document is approved; update `CLAUDE.md` to point to `docs/roadmap_v2.md` (or rename this file to `docs/ROADMAP.md`).

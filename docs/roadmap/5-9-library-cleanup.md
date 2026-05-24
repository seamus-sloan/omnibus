# F5.9 — Library cleanup

**Phase 5 · Admin & hygiene** · **Priority:** P2

One admin-only sanitization flow that detects and resolves near-duplicates, junk rows, and title cruft across authors, series, tags, and book titles. Surfaced from Settings as a Tinder-style Yes/No review queue, with companion on-page delete buttons on the author and series detail pages.

## Objective

After importing a third-party library (e.g. a Calibre dump), omnibus's normalized taxonomy is full of fragmentation: `Brandon Sanderson` / `Sanderson, Brandon`; `Sarah J. Maas` / `Maas, Sarah J` / `Maas, Sarah J.`; semicolon-soup tags like `Fantasy Romance; Fantasy New Adult; Fantasy; ...`; book titles carrying author and series prefixes (`Maas, Sarah J - A Court of Thorns and Roses 02 - A Court of Mist and Fury`); and ~80 junk-author rows from Calibre's `<dc:contributor opf:role="bkp">` tooling metadata.

This initiative ships:

1. A unified suggestion queue (`dedup_suggestions`) populated by a background detection job covering all four kinds.
2. A Tinder-style review UI at `/settings/cleanup/:kind` — one card at a time, Accept / Reject / Skip with keyboard hotkeys.
3. One transactional `merge`/`split`/`rename`/`delete` apply primitive per kind, all routed through a single `cleanup_log` audit table for 30-day undo.
4. An `entity_aliases` table so the next reindex doesn't silently re-create the merged-away rows.
5. Manual "Delete author" and "Delete series" buttons on the per-author and per-series detail pages for cases the detector misses.
6. Automatic orphan-cleanup — when a series or tag drops to zero linked books, it's pruned. (Authors are deliberately excluded.)

Closes [seamus-sloan/omnibus#159](https://github.com/seamus-sloan/omnibus/issues/159).

## User / business value

Unblocks:

- **Coherent author and series pages.** A fresh Calibre import leaves the same author scattered across two or three pages with partial bibliographies. Merging gives one canonical page per author with the full backlist.
- **Less fragmented search.** [F1.5](1-5-advanced-search.md) palette results for "Maas" currently surface three rows for one author. Merging collapses them.
- **First-import sanity pass.** A workbench surface turns the first 30 minutes after a bulk import from "scroll through hundreds of authors hunting visually" into "click through a pre-scored list."
- **Clean book titles.** Stripping filename cruft from `books.title` (via the existing [F5.1](5-1-metadata-edit.md) `metadata_overrides` path) makes covers, palette results, and series pages readable.

## Technical considerations

### Detection

Tier 0 = high confidence; Tier 1 = fuzzy. Both surface in the queue with a visible score — per user decision, fuzzy is not gated behind a toggle.

- **Authors / series / tags merge:** Tier 0 = `GROUP BY` on a normalized key (lowercase, strip punctuation, collapse whitespace, swap `Last, First` → `First Last` via `COALESCE(sort, name)`). Tier 1 = token-set Jaccard ≥ 0.85, blocked by shared first token to stay sub-quadratic.
- **Tags split:** heuristic — `name` contains `;` (tier 0) or is long with embedded commas (tier 1). Atoms come from splitting on the detected delimiter.
- **Book titles rename:** regex normalizer with patterns for `Last, First - `, `[SERIES NN] `, `Series #NN `, `ToG04-`, trailing parenthetical editions. Tier 0 for the high-cruft patterns, tier 1 for parenthetical-only.
- **Junk delete:** regex match against known tooling-artifact patterns (`^calibre \(`, `\[http`, `Smashwords, Inc\.`, etc.) emits `Action::Delete` suggestions, tier 0.

Runs as `Task::DetectCleanup { kind: Option<Kind> }` in the existing [F0.5 worker](0-5-background-worker.md). Triggered after every successful `indexer::reindex` and on admin demand from Settings.

### Schema

Three new tables: `dedup_suggestions` (queue + decision), `cleanup_log` (snapshot-based undo), `entity_aliases` (merged-name → canonical-id map consulted by `resolve_or_insert_*`). Full DDL lands with the numbered migration under [`db/migrations/`](../../db/migrations/) that introduces these tables.

### Apply primitives

In a new module `db/src/cleanup.rs`. Each `apply_*` is a single transaction that snapshots into `cleanup_log` first, then mutates. Common shape for merges: relink the join table (`INSERT OR IGNORE`), refresh `books_fts` for affected books, write source names to `entity_aliases`, delete sources. Author merges also reconcile `author_photos` (manual > openlibrary > letter) and backfill `sort` if NULL.

Book-title renames delegate to the existing `upsert_metadata_overrides` in `db/src/queries.rs` — title changes go through the JSON override blob, not direct `books.title` mutation.

### Reindex-resurrection guard

`resolve_or_insert_author` / `_series` / the tag insertion site in `db/src/queries.rs` each gain a one-line lookup against `entity_aliases` before the existing INSERT. Without this, the next `Task::Scan` silently undoes every merge.

### Orphan auto-cleanup

`db::queries::prune_orphans(pool) -> Result<(u64, u64)>` runs two sweeps: `DELETE FROM series WHERE id NOT IN (...)` and the same for tags. Called at the end of `indexer::reindex` and any `apply_*` that touches link tables. Authors deliberately excluded — junk-author handling goes through the detection queue's `Action::Delete` path so it gets the audit log and alias guard.

### Surfaces

- **Settings** — new "Library cleanup" section with per-kind counts + Review buttons + "Run detection now". Settings is already admin-gated at the RPC level (`settings.rs` uses `AdminUser`); no extra gating needed.
- **`/settings/cleanup/:kind`** — single-card review page with ✓/✗/→ hotkeys (Y/N/Space).
- **`/authors/:id`** + **`/series/:id`** — admin-only "Delete" button with confirmation modal, routed through the same `apply_delete_*` primitive.

## Dependencies

- [F0.5 Background worker](0-5-background-worker.md) — detection runs as a worker task.
- [F0.7 Per-route authorization](0-7-route-authorization.md) — merge / delete / split are admin-gated.
- [F1.11 Author profiles](1-11-author-profiles.md) — photo reconciliation depends on the `author_photos` table shape.
- [F5.1 Metadata edit](5-1-metadata-edit.md) — book-title renames write through the existing `metadata_overrides` path.

## Risks

- **Wrong-merge.** Admin accepts a fuzzy match that's actually two different people. Mitigation: 30-day undo via `cleanup_log` snapshot; tier shown on every card so low-confidence suggestions get more scrutiny.
- **Reindex resurrection.** Without the alias guard, the next reindex re-creates merged-away rows and silently undoes the cleanup. Mitigation: the `entity_aliases` lookup in `resolve_or_insert_*`; integration test asserts a fresh scan does not re-create a merged-away author.
- **FTS drift.** Forgetting to refresh `books_fts` after a link-table move leaves stale tokens. Mitigation: each `apply_merge_*` explicitly re-inserts FTS rows for affected book ids; test asserts FTS rows match `books_authors_link` after a merge.
- **Detection cost on huge libraries.** O(n²) Jaccard is fine at 1k authors, painful at 50k. Mitigation: first-token blocking pass.
- **Tag splits create orphan singletons.** If a `;`-delimited tag splits to atoms that already exist, we want to merge into the existing atom, not create a duplicate. `INSERT OR IGNORE INTO tags(name)` + the existing `UNIQUE COLLATE NOCASE` constraint handle this.

## Open questions

**Resolved:**

- **Scope of "sanitize book names"** — title normalization (strip prefixes) only, not duplicate-file detection. Per user.
- **Tag splits** — yes, handle both merge and split. Per user.
- **Tier 1 fuzzy visibility** — always in the queue with a visible score, not behind an opt-in toggle. Per user.
- **On-page delete + auto-orphan** — both included from day one (issue #159).
- **Authors excluded from auto-orphan** — yes, by default. A zero-book author may be intentional; junk authors go through the detection queue so they get logged and aliased.

**Unresolved:**

- **Undo window length** — currently 30 days. Storage cost is trivial (one JSON blob per merge), so longer is probably fine.
- **Mobile surface** — v1 is admin-web only. Mobile parity (REST routes + native UI) is a follow-up; the data layer compiles cleanly under the mobile feature gate but no UI is exposed.

## TODOs

### Schema and migration

**What:** Add `db/migrations/NNNN_library_cleanup.sql` creating `dedup_suggestions`, `cleanup_log`, and `entity_aliases`.

**Why:** Persistent queue for review decisions, snapshot-based undo log, and the alias map that stops merges from being silently undone by the next reindex.

**Context:** `dedup_suggestions` is `UNIQUE (kind, action, payload_json)` so re-running detection doesn't insert duplicate rows. `cleanup_log.snapshot_json` is a self-contained JSON blob so undo works even after unrelated schema changes. `entity_aliases` is keyed by `(kind, alias_name)` and consulted at insert time inside `resolve_or_insert_*`.

**Effort:** S
**Priority:** P0
**Depends on:** None.

### Shared types

**What:** `CleanupKind`, `CleanupAction`, `Decision`, `CleanupCounts`, `SuggestionCard` in `shared/src/lib.rs`.

**Why:** Travel over RPC, so they need serde derives and no server-only deps. `SuggestionCard` carries hydrated preview data (names, book counts, photo URLs) so the review UI renders in one round-trip.

**Effort:** S
**Priority:** P0
**Depends on:** Schema and migration.

### Detection module

**What:** New `db/src/cleanup.rs` with `detect_authors`, `detect_series`, `detect_tags_merge`, `detect_tags_split`, `detect_book_titles`, plus a `detect_all` dispatcher. Re-export from `db/src/lib.rs`.

**Why:** Five domain-specific algorithms feeding one unified queue. Splitting per-kind keeps each algorithm simple to test independently.

**Context:** Tier 0 = `GROUP BY` on a normalized key (authors/series/tags) or regex-match high-cruft patterns (book titles, junk authors). Tier 1 = token-set Jaccard ≥ 0.85, blocked by shared first token. Book-title regex passes match-and-strip `Last, First - `, `[SERIES NN] `, `Series #NN `, `ToG04-`, trailing parenthetical editions. Junk-author regex matches `^calibre \(`, `\[http`, `Smashwords, Inc\.` style tooling artifacts.

**Effort:** M
**Priority:** P0
**Depends on:** Schema and migration; Shared types.

### Apply primitives + undo

**What:** Transactional `apply_merge_authors`, `apply_merge_series`, `apply_merge_tags`, `apply_tag_split`, `apply_book_title_override`, and `apply_delete_<kind>` in `db/src/cleanup.rs`. Plus `undo(log_id)` that restores from snapshot.

**Why:** Every accepted suggestion or on-page delete routes through these. Centralizing the transaction is the only way to keep FTS, photos, link tables, and the candidate cache in sync.

**Context:** Each primitive snapshots affected rows into `cleanup_log.snapshot_json` *first*, then mutates. Merges relink the join table (`INSERT OR IGNORE`), refresh `books_fts` for affected book ids, write source names into `entity_aliases`, and delete sources. Author merges additionally reconcile `author_photos` (manual > openlibrary > letter) and backfill `sort` if NULL. Book-title renames delegate to the existing `upsert_metadata_overrides` in `db/src/queries.rs` (the `metadata_overrides` JSON blob is the canonical edit surface for titles, not direct `books.title` mutation).

**Effort:** L
**Priority:** P0
**Depends on:** Detection module.

### Reindex-resurrection guard

**What:** Extend `resolve_or_insert_author` (around `db/src/queries.rs:463`), `resolve_or_insert_series`, and the tag insertion site in `insert_book_taxonomy` to consult `entity_aliases` before the existing `INSERT … ON CONFLICT`.

**Why:** Without this, the next `Task::Scan` re-creates the merged-away rows and silently undoes the admin's work. This is non-negotiable — the whole feature breaks without it.

**Context:** One extra `SELECT canonical_id FROM entity_aliases WHERE kind = ? AND alias_name = ?` per resolve site. If hit, return that id; otherwise fall through to the existing insert path.

**Effort:** S
**Priority:** P0
**Depends on:** Schema and migration.

### Orphan auto-cleanup helper

**What:** `db::queries::prune_orphans(pool) -> Result<(u64, u64)>` that runs the series + tags orphan sweep, returning the deletion counts.

**Why:** Issue #159's auto-delete-when-empty requirement for series and tags. A single sweep at the end of each write path is cheaper than per-row `DELETE` triggers.

**Context:** Called at the end of `indexer::reindex` (covers book-removed-from-disk → orphaned series/tag) and from every `apply_*` that touches link tables. Authors are deliberately excluded — empty authors are handled via the detection queue's `Action::Delete` path so they get the audit log + alias guard.

**Effort:** S
**Priority:** P1
**Depends on:** None.

### Worker task + indexer trigger

**What:** Add `Task::DetectCleanup { kind: Option<Kind> }` to `db/src/worker.rs` (variant + dispatch + `resource_key` returning `Some("cleanup")` to serialize). Trigger from `indexer::reindex` on success.

**Why:** Detection runs out of band so the review UI reads from a cache, not a per-request expensive query. Auto-running after reindex means fresh suggestions appear without admin action after an import.

**Context:** Matches the existing `ResolveAuthorPhoto` task shape at `db/src/worker.rs:16-66`. `uses_scan_sem()` returns `false` (cleanup detection doesn't compete with scans).

**Effort:** S
**Priority:** P1
**Depends on:** Detection module.

### RPC endpoints + data wrappers

**What:** Six server functions in `frontend/src/rpc.rs` — `cleanup/counts`, `cleanup/queue`, `cleanup/decide`, `cleanup/detect`, `cleanup/undo`, `cleanup/delete-entity`. Typed wrappers in `frontend/src/data.rs` under both web and mobile feature gates (mobile data layer should compile cleanly even though no UI is exposed in v1).

**Why:** Wire the merge primitive to the frontend. Admin-gated via the existing `AdminUser` extractor pattern (or inline `user.is_admin` check matching the F1.11 pattern at `frontend/src/rpc.rs:274`).

**Context:** No new REST `/api/*` routes — cleanup is admin-web only for v1. Mobile parity is a follow-up.

**Effort:** M
**Priority:** P1
**Depends on:** Apply primitives.

### Settings page section

**What:** Add a "Library cleanup" `<section>` near the bottom of `frontend/src/pages/settings.rs` (after the existing `settings-field` blocks). Shows per-kind counts + Review buttons + a "Run detection now" button.

**Why:** The entry point. Settings is the natural home for admin hygiene tooling.

**Context:** Settings page is already admin-gated at the RPC level — no extra UI gating needed. Counts auto-refresh on mount via `data::cleanup_counts`.

**Effort:** S
**Priority:** P1
**Depends on:** RPC endpoints.

### Review page (Tinder card UI)

**What:** New `frontend/src/pages/cleanup_review.rs` at `Route::CleanupReview { kind: String }` (added to the enum in `frontend/src/lib.rs:28-53`). Single-card-at-a-time UI with three actions: Accept (✓ / Y), Reject (✗ / N), Skip (→ / Space).

**Why:** The Yes/No review surface — one decision at a time keeps cognitive load low and lets the admin blast through a large queue.

**Context:** Card content varies by `(kind, action)` — author merge shows side-by-side author cards with photos and book counts; tag split shows source → atom chips; book rename shows current title (struck through) → proposed title. Reuses Atrium card primitives from `frontend/src/components/atrium.rs`. Empty queue state: "No suggestions pending."

**Effort:** M
**Priority:** P1
**Depends on:** RPC endpoints.

### On-page delete buttons (issue #159)

**What:** Admin-only "Delete author" button in `frontend/src/pages/author.rs` and "Delete series" button in `frontend/src/pages/series.rs`, each with a confirmation modal.

**Why:** Issue #159's manual-delete requirement. Useful when the admin spots a junk row the detector missed, or wants to delete a real-but-empty author left after a merge.

**Context:** Both route through `POST /api/rpc/cleanup/delete-entity`, which reuses the `apply_delete_<kind>` primitive — so on-page deletes get the audit log + alias guard for free. Modal copy: "Delete '{name}'? This will remove the author from {N} books. The books themselves are not deleted."

**Effort:** S
**Priority:** P1
**Depends on:** Apply primitives; RPC endpoints.

### Tests

**What:** Inline `#[cfg(test)]` in `db/src/cleanup.rs` for detection, each apply primitive, undo round-trip, `prune_orphans` (including the "authors are excluded" assertion), and direct on-page delete. Integration tests in `server/src/backend.rs` style. Playwright spec at `ui_tests/playwright/tests/flows/cleanup.spec.ts` covering layout, review flow, on-page delete, error path.

**Why:** Destructive admin operations need end-to-end verification — unit tests alone can't catch FTS / photo reconciliation regressions across the full stack.

**Context:** Per `.claude/rules/03-unit-testing.md` and `.claude/rules/04-playwright.md`. The reindex-resurrection guard needs its own integration test (merge → run a second scan → assert the source row is *not* re-created).

**Effort:** M
**Priority:** P1
**Depends on:** Apply primitives; Settings section; Review page; On-page delete buttons.

## Status

Queued. Scoped in this doc; branch `gh-159/library-cleanup` exists with this scoping commit. No implementation yet.

---

[← Back to roadmap summary](0-0-summary.md)

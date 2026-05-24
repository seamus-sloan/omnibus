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

Three new tables: `dedup_suggestions` (queue + decision), `cleanup_log` (snapshot-based undo), `entity_aliases` (merged-name → canonical-id map consulted by `resolve_or_insert_*`). See the plan in `/Users/seamus/.claude/plans/i-didn-t-want-you-proud-cook.md` for full DDL.

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

Detailed step-by-step implementation plan lives at `/Users/seamus/.claude/plans/i-didn-t-want-you-proud-cook.md` and is being executed on branch `gh-159/library-cleanup`.

## Status

In progress on branch `gh-159/library-cleanup`.

---

[← Back to roadmap summary](0-0-summary.md)

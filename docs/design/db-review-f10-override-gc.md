# F10 — `metadata_overrides` reconcile / GC for orphans

Status: Decided (2026-06-28) — **Option C (soft-detach)** with both GC arms
(auto post-reindex + explicit library removal), a 30-day retention before
hard-purge, and the admin "unlinked" surface deferred to F3.2. Supersedes the
doc's original Option B recommendation: F2 (durable stored `uuid` +
relative-path `scan_key`) has since landed, removing the mass-orphan footgun
that made B's caution necessary. The chosen answers are recorded in
[Decision (resolved)](#decision-resolved); the Options and original
Recommendation below are retained as the considered alternatives.

---

## Decision (resolved)

Both decisions from [Decision required](#decision-required) are settled:

- **D1 — Disposition: soft-detach (Option C).** Orphaned override rows are
  marked with a new nullable `detached_at` column, not deleted. They are
  filtered out of the browse/discovery read path immediately, re-link (clear
  `detached_at`) when the book resolves again on a later reindex, and are
  hard-purged only after a long retention. This is the F3.2-aligned shape, so
  the same mechanism serves `user_ratings` / `user_journal_entries` later.
- **D2 — When GC runs + grace: both arms, 30-day retention.** Reconcile runs
  **both** post-every-reindex (best-effort, logged) **and** on explicit library
  removal. Both arms soft-detach — the explicit-removal arm does *not*
  hard-delete, keeping one uniform path. Detach is immediate on detection; the
  row is retained for **30 days**, then a long-retention sweep hard-purges it.
  On the reindex path a detached row re-links (clears `detached_at`) if its
  book resolves again; rows detached via explicit removal can't re-link (re-add
  mints fresh uuids — see the caveat below) and just age out.
- **Visibility: admin "unlinked edits" UI deferred to F3.2.** The detach + GC
  data layer and an `info`-level pruned-count log ship now; the admin surface
  lands with F3.2's "unlinked annotations" view so both present as one
  consistent surface. Recorded in
  [`docs/roadmap/3-2-ratings-journaling.md`](../roadmap/3-2-ratings-journaling.md).

**Why Option C over the doc's original Option B recommendation: F2 has
landed.** A library-root repoint now preserves every `books.uuid` (the reindex
diff matches on the relative-path `scan_key` —
`db/src/sync/books/changed.rs`), so an orphan genuinely means "the file is
permanently gone." The mass-orphan hazard that made B's hard-delete dangerous
is gone, so the deciding factors became F3.2 alignment and no-data-loss on a
wrong detach, both of which favour C. The per-reindex arm — the part B gated
behind F2 — is therefore safe to enable now.

**Accepted caveat.** Soft-detach gives no re-link recovery on the
explicit-removal path specifically: re-adding a removed library mints fresh
uuids, so those rows sit as "unlinked" until the 30-day sweep. This was an
accepted trade for a single uniform code path.

---

## Problem

`metadata_overrides` is keyed by `book_uuid TEXT NOT NULL PRIMARY KEY`
with **no FK to `books`** (`db/migrations/0007_metadata_overrides.sql:16-22`).
That is deliberate: the indexer wipes-and-rewrites `books` on every
Changed/Removed reindex, and the override layer must survive that cycle so
a user's hand edits aren't lost when a file's mtime bumps. The
module/comment contract spells this out at `sync_books` in
`db/src/sync/books.rs:77-80` ("`metadata_overrides` is intentionally not
touched").

The trade-off the contract creates: **nothing ever removes an override row
whose book is permanently gone.** The only deletes are user-driven
(`delete_metadata_overrides`, the `DELETE` at
`db/src/metadata_overrides/upsert.rs:175`) and merge-source-driven
(`db/src/merge/transaction.rs:409`). Neither the Removed-bucket cascade
(`sync_removed`, `db/src/sync/books.rs:125-163`, which deletes only
`books_fts` + `books`) nor the explicit library prune
(`prune_orphan_libraries`, `db/src/settings.rs:100-163`, which deletes
`books_fts` + `books` + `libraries`) touches `metadata_overrides`. So a
file that is permanently deleted, or a whole library root that is removed
in settings, leaves its override **row** behind forever.

Two consequences:

1. **Dead rows accumulate and slow the hot path.** Every browse/discovery
   count query `LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid`
   (`db/src/browse.rs:52,150,175`, `db/src/discovery/authors.rs:67`). The
   join itself is PK-indexed, but orphan rows inflate the table the
   planner scans on the override-extracted-subjects path.
2. **(Historical — resolved by F2.)** This finding was originally compounded
   by the path-based `stable_uuid` (`library_path + filename`): a library-root
   change re-keyed *every* uuid at once, orphaning every override row
   permanently with no possibility of re-link. **F2 has since landed** —
   `books.uuid` is now a durable stored value and the reindex diff matches on
   the relative-path `scan_key` (`db/src/sync/books/changed.rs`), so a repoint
   preserves every uuid and no longer mass-orphans anything. The remaining leak
   is the narrow, legitimate one: an individual file (or an explicitly-removed
   library) that is *actually* gone. That is exactly the safe condition the
   per-reindex GC needs — which is why the decision enables it.

Already handled (do not re-solve): orphan override **cover files**
(`override-<uuid>.<ext>`) are already swept. `delete_cover_files_for`
(`db/src/covers.rs:144-150`) unlinks both `<uuid>.<ext>` and the override
variant; `set_settings` calls it for pruned uuids
(`db/src/settings.rs:80-85`) and `sync_books` calls it for Removed uuids
(`db/src/sync/books.rs:110-114`). The gap is the **row**, not the file —
though a hard-delete path should still call `delete_override_cover`
(`db/src/metadata_overrides/upsert.rs:303`) defensively for uuids that
reach reconcile by a route other than those two sweeps.

**Why it bites a named feature.** F3.2 ratings & journaling
(`docs/roadmap/3-2-ratings-journaling.md`) mandates the *same* soft-ref
pattern for `user_ratings` / `user_journal_entries` — `book_uuid TEXT NOT
NULL`, no FK, no cascade (lines 32-44). When a book is pruned those rows
"become detached rather than deleted" (line 44) and the roadmap wants them
surfaced as "unlinked annotations" rather than silently lost (line 51).
F3.2 therefore inherits this exact orphan-forever shape, and whatever
reconcile/GC mechanism we build for `metadata_overrides` is the one F3.2
will reuse. Building the wrong shape now means redoing it under user-data
load.

---

## Decision required

Two coupled decisions, both for the operator:

**D1 — Disposition of an orphan: hard-delete vs. soft-detach.** When a
reconcile finds an override row whose `book_uuid` is in neither `books`
nor `merged_uuids`, does it (a) `DELETE` the row outright, or (b) mark it
`detached_at` and keep it, so it can re-link when the book reappears and be
surfaced as "unlinked" in the UI? Hard-delete matches the immediate
problem (dead rows, slow joins) and ships with zero schema change.
Soft-detach is the F3.2-aligned pattern but adds a column, a clear-on-relink
write, and a UI surface that doesn't exist yet.

**D2 — When GC runs, and the grace window.** Does reconcile run
post-*every*-reindex, or only on explicit library-root removal? And how
long after a book disappears before its override is eligible for GC? This
is the dangerous knob: a path-based `stable_uuid` re-key (F2) orphans every
row simultaneously, so an aggressive post-every-reindex GC with a short
grace window would **nuke every user edit on a single library move**. The
grace window exists to protect rows whose book is *legitimately detached
pending the next reindex* (mid-library-swap, a transient scan failure)
from being mistaken for permanent orphans.

These two are coupled: a long grace window matters far less under
soft-detach (a wrongly-detached row just re-links, no data lost) than under
hard-delete (a wrongly-deleted row is gone). The conservative combination
is *soft-detach + short grace*; the aggressive one is *hard-delete +
per-every-reindex + short grace*, which is a footgun next to F2.

---

## Options

All three use the same orphan predicate, which needs **no schema change**:

```sql
book_uuid NOT IN (SELECT uuid FROM books)
AND book_uuid NOT IN (SELECT uuid FROM merged_uuids)
```

`book_uuid` is the PK; `books.uuid` is `UNIQUE` and `merged_uuids.uuid` is
a PK, so both sub-selects are index-served. The `merged_uuids` arm is
load-bearing: a uuid that was cross-format-merged/attached into another
book has no `books.uuid` row but is a *legitimate* attachment, and must
survive — this mirrors the `resolve_book_id_by_uuid` fallback in
`db/src/books/get.rs:132-145`.

### Option A — Minimal prune on explicit library removal only

**How it works.** Extend `prune_orphan_libraries`
(`db/src/settings.rs:100-163`) with a chunked
`DELETE FROM metadata_overrides WHERE book_uuid IN (...)` over the
`orphan_uuids` it already collects (`:125-135`), inside the same
transaction. No post-reindex hook, no grace window, no new function on the
reindex path. Override cover files for those uuids are already swept by
`set_settings`' `delete_cover_files_for` call — no extra fs work.

- **Migration shape.** None. Pure code change in one function.
- **Blast radius.** One function, one tx, plus a test. The Removed-bucket
  cascade (`sync_removed`) is *not* covered, so a single permanently
  deleted file still leaks a row until its library is removed — but that
  row re-attaches for free if the file returns (same uuid), so the only
  permanent leak left is the library-removal one this option closes.
- **Pros.** Smallest possible change; zero risk of nuking edits on a
  library swap because it only fires when the operator *explicitly removed*
  the root. No grace-window judgement call.
- **Cons.** Doesn't reconcile single-file Removed orphans, doesn't address
  the F2 mass-orphan case (a path *change* runs prune for the old root —
  this actually does fire there, deleting every old-root override, which
  may be *too* aggressive without a grace window or detach). Not reusable
  for F3.2's "surface as unlinked" requirement.

### Option B — Hard-delete reconcile with a grace window (recommended)

**How it works.** Add `reconcile_orphan_overrides(pool, grace_secs) ->
Result<u64, MetadataOverridesError>` in
`db/src/metadata_overrides/upsert.rs`:

```sql
SELECT book_uuid FROM metadata_overrides
 WHERE updated_at < datetime('now', '-' || ? || ' days')
   AND book_uuid NOT IN (SELECT uuid FROM books)
   AND book_uuid NOT IN (SELECT uuid FROM merged_uuids)
```

The bound parameter is an integer day count: SQLite concatenates it into a
valid relative modifier (e.g. `7` → `'-7 days'`).

then `delete_overrides_for_uuids(tx, &uuids)` (chunked at 500, mirroring
`load_overrides_bulk` and `prune_orphan_libraries`) and a defensive
`delete_override_cover` per uuid off the runtime via `spawn_blocking`.
Returns the count for logging. Wire it in two places: (1) `set_settings` /
`prune_orphan_libraries` as in Option A (immediate, no grace — explicit
removal is unambiguous); (2) `reindex` in `db/src/indexer.rs` after
`sync::sync_books` (`:259`), best-effort/logged, gated behind a grace
window of **days** so a transient mid-swap detach is not GC'd.

The grace window reuses the existing `updated_at` column
(`0007_metadata_overrides.sql:21`) — `updated_at < datetime('now',
'-N days')` — so **no new column** is required. Note the semantic
caveat in [Open questions](#open-questions): `updated_at` is *last-edit*
time, not *detach* time.

- **Migration shape.** None. The orphan + grace predicate runs entirely
  off the existing schema. (An optional, redundant `idx_metadata_overrides_uuid`
  is *not* needed — the PK already covers the join.)
- **Blast radius.** New `pub fn` + private helper in `upsert.rs`, a
  re-export in `mod.rs:21-25`, one best-effort call site in `indexer.rs`,
  the `settings.rs` prune extension, a doc-comment touch at
  `sync/books.rs:77-80`. Reuses `MetadataOverridesError::Db` — no new
  `thiserror` variant (the failure space is just `sqlx`, predictable, so
  `thiserror` is correct per rule 02).
- **Pros.** Closes both leak paths (Removed cascade *and* library prune),
  reusable shape for F3.2, idempotent, no schema migration, cheap once
  caught up. The grace window blunts the F2 mass-orphan footgun.
- **Cons.** Hard-delete is irreversible — a wrongly-GC'd row is gone, and
  the grace window is the only guard. Does *not* deliver the F3.2 "surface
  as unlinked" UX; F3.2 would still have to add `detached_at` later and
  decide whether to retrofit it onto `metadata_overrides`. The `updated_at`
  grace proxy is imperfect (see Open questions).

### Option C — Soft-detach (`detached_at`), F3.2-aligned

**How it works.** Add an append-only migration
`NNNN_metadata_overrides_detached.sql` (the next free number, allocated at
implementation time):

```sql
ALTER TABLE metadata_overrides ADD COLUMN detached_at TEXT;  -- NULL = attached
```

Reconcile *sets* `detached_at = datetime('now')` on first pass that finds
no book, *clears* it (back to NULL) when a re-index makes the book resolve
again, and a separate, much longer sweep hard-deletes rows whose
`detached_at` is older than a long retention (e.g. 90 days). Browse/discovery
joins gain `AND mo.detached_at IS NULL` so detached rows stop affecting the
read path immediately without being destroyed. The UI surfaces detached
overrides as "unlinked."

- **Migration shape.** One forward-only `ALTER TABLE … ADD COLUMN`,
  nullable, **no backfill** (existing rows default NULL = attached). This
  is a plain column add — *not* a PK/constraint change — so the SQLite
  pre-3.35 table-recreate dance (create-new, copy, drop, rename) does
  **not** apply here. Forward-only per rule 06.
- **Blast radius.** Migration + the reconcile fn (now an UPDATE-then-maybe-DELETE
  state machine) + every browse/discovery join gains a `detached_at IS NULL`
  predicate (`browse.rs:52,150,175`, `discovery/authors.rs:67`) + a UI
  surface + the re-link-clears-detached_at write on the reindex path. The
  largest of the three.
- **Pros.** No data loss on a wrong detach — rows re-link. Directly the
  pattern F3.2 wants; build once, reuse for ratings/journals. Read path is
  protected immediately (filtered out) while data is retained.
- **Cons.** Most code and the only schema migration. Adds a predicate to
  four hot queries. Needs a UI surface that doesn't exist yet to be
  user-visible — without it, detached rows are invisible *and* retained,
  i.e. all cost, deferred benefit. Over-builds for the immediate problem
  (dead rows slowing joins) if F3.2 slips.

---

## Recommendation

> **Superseded by the [Decision (resolved)](#decision-resolved): the operator
> chose Option C, not B.** This section is the doc's *original* analysis,
> retained for context. It recommended B because F2 had not yet landed; once
> F2 shipped (durable uuid + `scan_key`), the mass-orphan risk that justified
> B's caution disappeared and C's F3.2 alignment won. Read it as "why B *was*
> the conservative call," not as current guidance.

**Adopt Option B (hard-delete reconcile + grace window) now**, structured
so Option C is a clean follow-on rather than a rewrite.

Rationale:

- The immediate, named pain (dead rows, slow browse/discovery joins, a
  permanent leak on library removal) is fully closed by B with **zero
  schema migration** — the lowest-cost, lowest-risk fix that actually
  solves today's problem.
- B's grace window directly defuses the one genuinely dangerous
  interaction (F2 mass re-key orphaning every row at once): gate the
  per-reindex GC behind a grace window of days, and have the explicit-removal
  path fire immediately (an operator who removed a root meant it).
- **Reversibility / cost of delay.** B is cheap to *extend* into C: the
  `reconcile_orphan_overrides` choke-point, the orphan predicate, the
  chunked delete helper, and the call sites are all exactly what C needs.
  Adding `detached_at` later is a pure additive migration plus swapping the
  reconcile body from DELETE to UPDATE-then-retention-DELETE. Nothing in B
  has to be torn out. So choosing B does **not** foreclose C.
- The one thing B gives up vs. C — "surface as unlinked" and no-data-loss
  on wrong detach — is a *user-data* concern that only becomes real when
  F3.2 ships ratings/journals. Until there is durable, non-regenerable user
  data behind the soft-ref, hard-delete of a *regenerable-by-re-edit*
  metadata override is an acceptable loss. (An override can be re-entered;
  a journal entry cannot.) Building C's machinery before its consumer
  exists is speculative.

**Explicit guidance to ship with B:** make the per-reindex grace window a
named constant (suggest 7 days) and the explicit-removal path immediate;
log the GC count at `info`; and add a one-line note in
`docs/roadmap/3-2-ratings-journaling.md` that user-data tables must adopt
`detached_at` (Option C) rather than hard-delete, since their rows are not
regenerable. **Do not** point the per-reindex GC at the future user-data
tables — those must wait for C.

If the operator expects F3.2 to land *imminently* and wants to avoid two
migrations on adjacent tables, jumping straight to C is defensible — but
that is the only condition under which C beats B today.

---

## Migration plan

> **SKETCH — not to be applied.** The chosen option is **C (soft-detach)**, so
> the single `ALTER TABLE … ADD COLUMN detached_at` migration (under
> [Option C](#option-c--soft-detach-detached_at-f32-aligned) below) **does**
> apply, alongside the read-path `detached_at IS NULL` filter and the
> UPDATE-then-retention-DELETE reconcile state machine. The "no migration"
> sketch immediately below is Option B's, retained as the considered
> alternative.

**Option B (the considered alternative): no migration.** All work is in Rust:

1. `db/src/metadata_overrides/upsert.rs`
   - `pub(crate) async fn delete_overrides_for_uuids(tx, uuids: &[String]) -> Result<(), sqlx::Error>`
     — chunked at 500, `DELETE FROM metadata_overrides WHERE book_uuid IN (...)`.
   - `pub async fn reconcile_orphan_overrides(pool, grace_secs) -> Result<u64, MetadataOverridesError>`
     — SELECT orphans (predicate above + `updated_at < datetime('now', '-' || ? || ' days')`),
     `delete_overrides_for_uuids`, defensive `delete_override_cover` per uuid
     off-runtime via `spawn_blocking` (mirror `settings.rs:80-85`). Reuse
     `MetadataOverridesError::Db`.
2. `db/src/metadata_overrides/mod.rs` — re-export `reconcile_orphan_overrides`
   in the `pub use` block (`:21-25`); update the `//!` doc to mention GC.
3. `db/src/settings.rs` — in `prune_orphan_libraries`, after collecting
   `orphan_uuids`, add the chunked `DELETE FROM metadata_overrides WHERE
   book_uuid IN (...)` inside the same tx (no grace — explicit removal).
4. `db/src/indexer.rs` — in `reindex`, after `sync::sync_books` (`:259`),
   call `reconcile_orphan_overrides(pool, GRACE_SECS)` best-effort; log the
   returned count, never fail the reindex (matches the best-effort
   cover/FTS pattern).
5. `db/src/sync/books.rs:77-80` — update the comment to note overrides are
   now reconciled out-of-band; the sync path still intentionally doesn't
   touch them inline.

**Option C (only if chosen): one append-only migration.**

```sql
-- db/migrations/NNNN_metadata_overrides_detached.sql
-- Soft-detach marker for override rows whose book has disappeared.
-- NULL = attached (the default for every existing row, so no backfill).
-- Set on first reconcile that finds no books/merged_uuids match; cleared
-- back to NULL when a reindex makes the book resolve again. Plain ADD
-- COLUMN (not a PK/constraint change) — no table-recreate needed.
ALTER TABLE metadata_overrides ADD COLUMN detached_at TEXT;
```

No boot backfill is required for C (NULL default *is* the correct
"attached" state). If one were ever needed, it would follow the idempotent
`backfill_norm_columns` model (`db/src/normalize.rs:53-82`, called from
`init_db` in `db/src/pool.rs:56`): guarded on `detached_at IS NULL`, a
no-op once caught up, run on every boot. We deliberately do **not** add one
here.

---

## Affected code

From the scout spec, for Option B:

| File | Symbol(s) |
|---|---|
| `db/src/metadata_overrides/upsert.rs` | NEW `reconcile_orphan_overrides`; NEW `delete_overrides_for_uuids`; reuse `MetadataOverridesError`, `delete_override_cover` |
| `db/src/metadata_overrides/mod.rs` | re-export `reconcile_orphan_overrides`; `//!` doc update |
| `db/src/settings.rs` | `prune_orphan_libraries` — add chunked override DELETE in-tx; `set_settings` cover sweep already covers files |
| `db/src/indexer.rs` | `reindex` — best-effort `reconcile_orphan_overrides` call after `sync::sync_books` (`:259`) |
| `db/src/sync/books.rs` | comment at `:77-80` — note out-of-band reconcile |
| `db/src/metadata_overrides/tests.rs` | new reconcile + `delete_overrides_for_uuids` tests |
| `db/src/settings.rs` (tests mod) | prune-deletes-override-rows test |

For Option C additionally: `db/migrations/NNNN_metadata_overrides_detached.sql`;
`detached_at IS NULL` predicate on `db/src/browse.rs:52,150,175` and
`db/src/discovery/authors.rs:67`; clear-on-relink write on the reindex path.

---

## Test plan

Per rule 03: sibling-file tests against `sqlite::memory:` via the crate's
`test_support` pool init; happy path + one per failure mode.

In `db/src/metadata_overrides/tests.rs` (existing sibling file):

- `reconcile_orphan_overrides_deletes_row_when_book_and_merged_uuid_absent`
  — **the acceptance test that must fail on the old schema/code**: seed a
  book + override, drop the book, assert reconcile (grace=0) removes the
  override row and returns count 1. On `main` (no reconcile fn) this can't
  even compile/pass — it is the red test.
- `reconcile_orphan_overrides_keeps_row_when_book_present` — override for a
  live book survives.
- `reconcile_orphan_overrides_keeps_row_when_merged_uuid_present` — uuid
  only in `merged_uuids` (cross-format attachment) survives; guards the
  `resolve_book_id_by_uuid` fallback shape.
- `reconcile_orphan_overrides_respects_grace_window` — orphan with recent
  `updated_at` survives a non-zero grace; an old one is GC'd.
- `reconcile_orphan_overrides_unlinks_override_cover_file` —
  `write_override_cover` then orphan + reconcile; assert
  `find_override_cover_file` returns `None`.
- `reconcile_orphan_overrides_propagates_db_error_when_pool_closed` — the
  one `MetadataOverridesError::Db` variant path.
- (only if `delete_overrides_for_uuids` is the chunked bulk helper)
  `delete_overrides_for_uuids_chunks_over_500` — happy path, large vec.

In `db/src/settings.rs` inline tests mod:

- `prune_orphan_libraries_deletes_override_rows_for_pruned_books` — seed
  two libraries each with an override, prune one via `set_settings`, assert
  the pruned library's override rows are gone and the kept library's
  remain.

If Option C is chosen, add: an orphan flips `detached_at` non-NULL; a
re-indexed book clears it back to NULL; browse counts exclude detached
rows.

---

## Risks & rollback

- **Forward-only, fix-forward.** Per rule 06 there are no down-migrations.
  Option B has no migration to roll back at all. Option C's `ADD COLUMN` is
  irreversible in place; a regret is corrected by a *new* migration, never
  by editing the `NNNN` one once it has run.
- **Data-loss surface (the real risk).** Option B hard-deletes. The only
  guard against a wrong delete is the grace window + the `merged_uuids`
  arm. The acute failure is the **F2 interaction**: if `stable_uuid`
  re-keys on a library-path change while the per-reindex GC has a short
  grace, every pre-change override is eligible and gets wiped. Mitigations:
  (1) keep the per-reindex grace at days, not minutes; (2) keep the
  explicit-removal path the only *immediate* GC; (3) **sequence F10 after
  F2** so the uuid is stable before GC runs on the reindex path (see below).
- **Irreversible once data accumulates.** Whatever disposition ships
  becomes load-bearing once F3.2 user data exists behind the soft-ref —
  retrofitting from hard-delete to soft-detach *after* journals are being
  GC'd would mean having already lost detached journal entries. This is the
  core reason the recommendation is "B now, but do not point per-reindex GC
  at user-data tables until C lands."
- **Best-effort GC must never fail a reindex.** The `indexer.rs` call site
  logs and swallows errors, matching the existing best-effort cover/FTS
  pattern. A GC failure leaving stale rows is strictly better than a GC
  failure aborting an index.

---

## Sequencing & dependencies

This finding sits in the F1↔F2↔F10 user-data-durability chain and the scout
flags it co-dependent with F2, F3, F6, F8, F11, F12, F13, F16, F17, F18.
The load-bearing edges:

- **F2 has landed → the per-reindex GC arm is enabled.** F2 shipped as a
  durable stored `books.uuid` plus a relative-path `scan_key` diff (not the
  earlier `dc:identifier`/content-hash proposal, which was dropped). A
  library-root repoint now preserves every uuid, so an orphan genuinely means
  "the file is gone" and the per-reindex GC is safe to run — which is why the
  decision enables it rather than gating it off behind explicit removal.
- **F1 (migrate the five user-data tables to `book_uuid` soft-refs)** is
  the sibling problem: those tables have the *opposite* bug today
  (cascade-delete on reindex). F10's reconcile is the GC half of the
  soft-ref pattern F1 introduces. Order: F1 converts the tables → F10/C
  provides their reconcile. F10 for `metadata_overrides` (already soft-ref)
  can proceed independently of F1.
- **F3.2 ratings/journaling is the consumer** that decides whether C is
  worth its cost. If F3.2 is near, prefer C to avoid two migrations on
  adjacent tables. If F3.2 is far, ship B and defer C.

Minimum safe slice if nothing else has landed: **Option A** (explicit-removal
prune only) — it has no F2 dependency because it fires only when the
operator removed a root, where mass-orphaning is intended.

---

## Open questions

All five are now resolved by the [Decision (resolved)](#decision-resolved);
kept here with their resolutions for the record.

1. **Grace-window semantics — RESOLVED.** Option C adds a real `detached_at`
   column, so grace is measured from *detach* time, not the imperfect
   `updated_at` last-edit proxy. The original concern (grace protecting
   recently-*edited* rather than recently-*detached* rows) no longer applies.
2. **Grace / retention length — RESOLVED: 30 days.** Detached rows are kept
   and re-linkable for 30 days before the long-retention sweep hard-purges
   them. (The earlier "7 days" proposal was for B's hard-delete grace; under
   soft-detach the equivalent knob is the retention-before-purge, set to 30.)
3. **Should the explicit-removal arm get a grace? — RESOLVED.** It soft-detaches
   like the scan path (no immediate hard-delete) and shares the same 30-day
   retention, for one uniform code path. Accepted caveat: those rows can't
   re-link on re-add (fresh uuids), so they sit "unlinked" until the sweep.
4. **F3.2 retention under C — RESOLVED for `metadata_overrides`: 30 days.**
   Per-table configurability and the user-data tables' own retention are F3.2's
   to set when it adopts this mechanism; F10 fixes only the override table.
5. **Observability — RESOLVED.** `info`-level pruned-count log ships now; the
   admin-visible "N orphaned overrides pruned" / "unlinked" surface is deferred
   to F3.2 so it lands as one consistent view with "unlinked annotations".

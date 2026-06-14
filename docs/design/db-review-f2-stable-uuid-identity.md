# Durable book identity — re-anchor `stable_uuid` off path

Status: Proposed — deferred from db.md F2, awaiting decision.

This doc frames the data-model decision that must be made *before* any
schema or code is written. It does not prescribe DDL to apply today — the
migration section is a sketch contingent on the decision below. Companion
findings: F1 (user-data soft-refs) and F10 (override GC / reconcile).

## Problem

A book's durable identity is derived from its filesystem location, not its
content. `stable_uuid(library_path, filename)` is
`Uuid::new_v5(NAMESPACE_URL, "{library_path}\0{filename}")` —
`db/src/helpers.rs` (`stable_uuid`). The uuid is minted in **Phase A**, the
pure filesystem stat that never opens the zip: `db/src/ebook/stat.rs`
stamps `StatEntry.uuid` from `stable_uuid(library_path_key, &relative)`.
That uuid is then the join key for the *entire* incremental diff —
`diff_library` in `db/src/indexer.rs` keys disk-vs-DB rows on it
(`disk_by_uuid` / `db_by_uuid`), and the sync writers
(`insert_book_row`, `sync_changed`, `try_attach_new_ebook`,
`attach_ebook_file` in `db/src/sync/books.rs`) recompute the same value to
locate the row they are about to write.

Because the key embeds `library_path`, **any change to the library root
re-mints a brand-new uuid for every book**: a settings edit that points the
ebook library at a new path, the F0.6 filesystem reorg, or simply moving the
data directory. Settings prune then hard-deletes the old `books` rows
(`prune_orphan_libraries`, `db/src/settings.rs`), the next scan inserts
fresh rows under new uuids, and everything keyed on the old uuid silently
detaches:

- `metadata_overrides.book_uuid` (PK, no FK — `0007_metadata_overrides.sql`).
- `merged_uuids.uuid` (PK — `0016_format_merge.sql`), the cross-format
  attach/merge ledger.
- Cover and thumbnail files named `<uuid>.<ext>`, and every
  `/api/covers/{uuid}` / `/api/thumbs/{uuid}` URL.
- The resolver `resolve_book_id_by_uuid` (`db/src/books/get.rs`) returns
  `None`, so old detail-page links and in-flight progress POSTs 404.

This bites the roadmap directly. F3.2 ratings & journaling
(`docs/roadmap/3-2-ratings-journaling.md`) specs user-data tables that
soft-reference `book_uuid TEXT` precisely so a pruned book's rating
*detaches and auto-relinks* when the file reappears. That contract is a lie
while the uuid itself moves on a path change: the row detaches and never
relinks. The roadmap lists the fix as a hard pre-req — "the current
`stable_uuid(library_path, filename)` scheme breaks this … must land before
this feature ships."

Two facts the original review under-stated, both load-bearing for the fix:

1. **The anchor is not available in Phase A.** The OPF `dc:identifier` is
   parsed only in Phase B (`collect_identifiers` / `doc.unique_identifier`
   in `db/src/ebook/parse.rs`), which opens the zip. The uuid is needed in
   Phase A as the diff key. So we cannot "just re-derive `books.uuid` from
   `dc:identifier`" without either opening every zip on every scan (killing
   incremental-scan perf) or splitting identity into two keys.
2. **`dc:identifier` is parsed but never persisted.** There is no DB column
   for it. `db/src/books/projection.rs` (`row_to_ebook`) sets
   `unique_identifier: Some(uuid.clone())` — it just echoes `books.uuid`.
   The "primary anchor" the roadmap names is not even stored yet.

Audiobooks have no `dc:identifier` at all (folder-grouped MP3/M4B —
`stable_uuid(library_path_key, &dir)` in `db/src/audiobook/group.rs`), so any
`dc:identifier`-primary scheme is ebook-only; audiobooks need a content-hash
anchor. `sha2::Sha256` is already a `db` dependency
(`db/Cargo.toml`, used by `db/src/auth/token.rs`), so the hash fallback needs
no new crate.

## Decision required

Four choices, in priority order. The first is the deep one and gates the
rest. None is cheaply reversible once real cover files, merged_uuids rows,
and (post-F1) user data accumulate under the chosen scheme.

1. **Two-key split vs. re-key `books.uuid` in place.** Keep a cheap
   path-derived **scan key** for the Phase-A diff and add a separate durable
   **identity key** resolved in Phase B? Or make `books.uuid` itself the
   durable identity, forcing a table-recreate and a boot re-key that breaks
   every cover filename, `merged_uuids` row, and override at once?
2. **Anchor precedence and the "unstable id" detector.** Proposed:
   `dc:identifier` (when stable) > SHA-256 of file bytes > scan-key
   fallback. Confirm the rule for rejecting an unstable `dc:identifier`
   (per-export random `urn:uuid:` ids — many EPUB editors mint a fresh one
   each export), and whether the content hash is computed at Phase-A stat
   time (re-reads every file — costly) or only for New/Changed books in
   Phase B.
3. **Audiobook anchor.** No `dc:identifier` exists. Content-hash of the
   first part? A hash of all part sizes+names? Decide separately from the
   ebook scheme.
4. **Reconcile / relink policy.** When the identity moves anyway (a true
   re-export with a new id and changed bytes), how are detached
   `metadata_overrides` / `merged_uuids` / (post-F1) user-data rows
   re-linked — by `(author, title)` similarity — and are unmatched rows
   surfaced as "unlinked" in the UI or silently dropped?

## Options

### Option A — Additive identity column, scan-key stays the diff key (two-key split)

How it works. `books.uuid` keeps its current `stable_uuid(library_path,
filename)` semantics and remains the Phase-A diff key — `diff_library` and
the sync writers are unchanged. Add `books.identity_uuid TEXT` (nullable)
and `books.content_hash TEXT`, both resolved in **Phase B** where the OPF is
already open. A new helper `book_identity_uuid(opf_identifier, content_hash,
scan_key)` picks: stable `dc:identifier` → UUIDv5 over it; else SHA-256 of
file bytes; else fall back to the scan key so a parse-less file still gets a
value. `metadata_overrides` and the F1 user-data tables move to reference
`identity_uuid`; `merged_uuids` and cover files migrate to it over a
transition window during which `resolve_book_id_by_uuid` UNIONs both keys.

Migration shape. Purely additive — `ALTER TABLE books ADD COLUMN
identity_uuid`, `ADD COLUMN content_hash`, plus a partial unique index. No
table-recreate. A boot backfill (`backfill_identity_uuids`, mirroring
`backfill_norm_columns`) populates the new columns for pre-migration rows.
Forward-only, idempotent, complies with rule 06.

Blast radius. New columns + one helper + Phase-B persistence in the sync
writers + a dual-resolver UNION + a boot backfill + a cover-file relink. The
hot Phase-A diff path is untouched, so incremental-scan perf is unchanged.
Two identity concepts coexist (scan key for diffing, identity key for
durability), which is conceptual overhead but each has a single clear job.

Pros. Lowest risk; reversible (drop the columns, nothing else moved);
incremental scan perf preserved; no force-recompute of existing cover URLs
on day one. Cons. Two columns to reason about forever; `books.uuid` keeps a
misleading name (it is the scan key, not the identity); a later cleanup
migration is needed if you ever want to retire the legacy key.

### Option B — Re-key `books.uuid` in place to the durable identity (one key)

How it works. `books.uuid` *becomes* the durable identity (dc:identifier /
content-hash). A second, non-durable column (`books.scan_key`) takes over
the Phase-A diff role. Every consumer of "the uuid" — covers, routes,
`merged_uuids`, overrides, F1 user data — now references the durable value
directly; no dual-resolver needed long-term.

Migration shape. SQLite cannot `ALTER` a `UNIQUE`/derivation in place, so
this is the **table-recreate dance**: `CREATE books_new` with the new
semantics, `INSERT … SELECT`, `DROP`, `RENAME`, recreate every `idx_books_*`
and re-point child tables. Critically, **the new uuid cannot be computed in
SQL** — it needs `dc:identifier` from the OPF / the file bytes, neither in
the DB. So the `.sql` can only add the column; the actual re-key must happen
in Rust at boot (open each file, read/hash, UPDATE, re-point
`merged_uuids` + `metadata_overrides` + rename `<old>.<ext>` →
`<new>.<ext>`).

Blast radius. Largest. Recreate `books`, re-point all uuid-keyed children,
re-key every cover file, and run a one-shot boot migration that opens every
file in the library. Any failure mid-migration leaves a partially re-keyed
DB. Every external `/api/covers/{old-uuid}` URL breaks at the cutover unless
the dual-resolver is kept anyway.

Pros. One canonical identity; clean long-term model; `unique_identifier`
projection becomes truthful for free. Cons. High-risk irreversible
migration; opens every file at boot; force-breaks cover URLs and
`merged_uuids` simultaneously; the table-recreate must be perfectly ordered
against FKs; effectively still needs Option A's dual-resolver during the
cutover, so it is "Option A plus a destructive recreate."

### Option C — Defer: ship F1 soft-refs now, block F3.2 on F2

How it works. Recognize that the co-dependency with F1 is **currently
latent**: the five user-data tables still key on numeric `book_id`
(`0013`, `0017`), so today only `metadata_overrides` + `merged_uuids`
actually detach on a re-key. Land F1 (move user data to `book_uuid`
soft-refs) first, then revisit F2 with the full set of durable consumers
known. F3.2 stays blocked until F2 lands, per the roadmap.

Migration shape. None for F2 now. Cons. Does not solve the problem; just
sequences it. Including it here only to make explicit that F2 *can* slip
behind F1 without new data loss — but every day F3.2 is blocked is a day the
"my library, my notes" pitch is unbuildable.

## Recommendation

**Option A (additive identity column, two-key split).**

Rationale. The architectural blocker — the anchor is unavailable in Phase A
— makes the two-key split not a stylistic preference but the shape the
indexer forces on us. We *need* a cheap path-derived key for the diff
regardless; Option B keeps that key too (renamed `scan_key`) and pays for a
destructive table-recreate on top. The marginal benefit of B (one canonical
column, a truthful `unique_identifier`) does not justify a boot-time
re-key that opens every file and force-breaks every cover URL and
`merged_uuids` row at once, on a schema whose user-data tables haven't even
moved to uuids yet (F1).

Reversibility / cost-of-delay. Option A is the only reversible choice: until
the legacy key is retired, dropping `identity_uuid`/`content_hash` restores
the status quo. Once F3.2 ratings and F1 user data accumulate under *any*
scheme, the choice hardens — re-keying live ratings/journals by hand is
exactly the irreversible cost the roadmap warns about. So the cheap,
additive, reversible model is the right one to commit to *before* that data
exists. The path to B remains open later (retire the legacy key in a future
migration) if the two-column model proves confusing — but we never have to
take that risk to unblock F3.2.

## Migration plan (SKETCH — not to be applied yet)

Next free number is `0019` (highest applied is
`0018_multiformat_book_files.sql`). Forward-only, append-only per rule 06.

```sql
-- 0019_book_identity_uuid.sql  (SKETCH — additive identity model)
ALTER TABLE books ADD COLUMN identity_uuid TEXT;   -- nullable; backfilled at boot
ALTER TABLE books ADD COLUMN content_hash  TEXT;   -- SHA-256 hex of file bytes

-- Durable identity is unique among rows that have one; NULLs (pre-backfill,
-- or files that never parsed) are exempt via the partial index.
CREATE UNIQUE INDEX idx_books_identity_uuid
    ON books(identity_uuid) WHERE identity_uuid IS NOT NULL;
```

The `.sql` is additive only. The actual identity derivation **cannot** live
in SQL (it needs the OPF / file bytes), so a boot backfill in Rust does the
work, mirroring `normalize::backfill_norm_columns` and wired into `init_db`
in `db/src/pool.rs` right after it:

```rust
// db/src/normalize.rs (or a new identity.rs) — SKETCH
// Idempotent: only touches rows WHERE identity_uuid IS NULL.
// Gated off in-memory DBs the same way the #94 cover purge is in
// init_db, so the rapid-fire test suite doesn't hit the filesystem.
pub async fn backfill_identity_uuids(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    // SELECT id, library_path, filename FROM books WHERE identity_uuid IS NULL
    // for each: open file -> read dc:identifier / hash bytes ->
    //   identity = book_identity_uuid(opf_id, hash, existing_uuid)
    //   UPDATE books SET identity_uuid = ?, content_hash = ? WHERE id = ?
    //   re-point metadata_overrides.book_uuid + merged_uuids.uuid whose old
    //   value no longer resolves but whose (author,title) matches this row
    //   rename covers/<old-uuid>.<ext> -> covers/<identity_uuid>.<ext>
    Ok(())
}
```

Covers are a rebuildable cache, so a missed rename regenerates on the next
reindex — the rename is best-effort, like the existing `#94` purge. A
**later** migration (`0020+`) can re-key `merged_uuids` / `metadata_overrides`
PKs to `identity_uuid` once it is the canonical key; do not do that in `0019`.

## Affected code

From the scout spec (current symbols, not line anchors — anchors rot):

- `db/src/helpers.rs` — keep `stable_uuid` as the scan key (consider
  renaming to `scan_key_uuid`); add `book_identity_uuid(opf_identifier,
  content_hash, scan_key)` and `is_unstable_opf_identifier()` (detects
  per-export `urn:uuid:` ids).
- `db/src/ebook/parse.rs` — `extract_metadata` already collects
  `identifiers` + `unique_identifier`; add the content-hash hook only if
  hashing happens here.
- `db/src/ebook/stat.rs` — `StatEntry.uuid` stays the path scan key (Phase A
  can't see the OPF); no identity change here.
- `db/src/sync/books.rs` — `insert_book_row`, `sync_changed`,
  `try_attach_new_ebook`, `attach_ebook_file`: compute and persist
  `identity_uuid` + `content_hash`; keep the scan key for diff and
  `merged_uuids` matching.
- `db/src/indexer.rs` — `diff_library` keeps matching on the scan key;
  `reindex` gains a reconcile/relink pass after `sync_books`.
- `db/src/books/get.rs` — `resolve_book_id_by_uuid` UNION extended to resolve
  by `identity_uuid` AND legacy `books.uuid` AND `merged_uuids` during the
  transition.
- `db/src/books/projection.rs` — `row_to_ebook` surfaces the persisted
  `identity_uuid` as `unique_identifier` instead of echoing `books.uuid`.
- `db/src/audiobook/group.rs` — content-hash anchor (no `dc:identifier`).
- `db/migrations/0019_book_identity_uuid.sql` — additive columns + partial
  unique index.
- `db/src/normalize.rs` (or new `identity.rs`) — `backfill_identity_uuids`,
  wired into `db/src/pool.rs` `init_db`.
- `db/src/test_support.rs` — seed factories updated for the new scheme.
- `db/src/helpers/tests.rs` — new identity-derivation tests.

## Test plan

Per rule 03: sibling `<mod>/tests.rs`, all against `sqlite::memory:`. The
**acceptance test that must fail on the old schema** is the root-change
survival test below — on today's code the identity moves, so it fails until
the durable anchor lands.

- `db/src/helpers/tests.rs` — `book_identity_uuid`: stable `dc:identifier`
  wins (happy); falls back to SHA-256 when the id is absent/empty;
  falls back to the scan key when no file/hash; determinism + UUIDv5
  version/variant checks mirroring the existing `stable_uuid_*` tests;
  `is_unstable_opf_identifier` detects per-export `urn:uuid:` ids.
- `db/src/sync/tests.rs` — **acceptance:** seed a book, reindex under a *new*
  `library_path`, assert the SAME `identity_uuid` and that
  `metadata_overrides` + `merged_uuids` still resolve. Assert
  `insert_book_row` / `sync_changed` / the attach paths all persist
  `identity_uuid`.
- `db/src/books/tests.rs` — `resolve_book_id_by_uuid` resolves by
  `identity_uuid`, by legacy path-uuid, and by `merged_uuids` (one happy per
  branch).
- Backfill test — seed a row with NULL `identity_uuid` (simulate pre-0019),
  run `backfill_identity_uuids` twice, assert it populates once and is a
  no-op the second time (idempotent, like the `backfill_norm_columns` tests).
- Reconcile test — detach a `metadata_overrides` row via a root change, run
  `reindex`, assert it re-links by `(author, title)` to the surviving
  `identity_uuid`.
- Audiobook test — content-hash anchor stable across a root change for an MP3
  group (`db/src/audiobook` tests).
- Migration test — assert `0019` adds the column + partial unique index and
  that a duplicate `identity_uuid` is rejected. The `MIGRATOR` runs `0019`
  in every existing memory-DB test, so the migration is already exercised.

## Risks & rollback

- **Forward-only / fix-forward.** No down-migrations (rule 06). A bug in
  `0019` is corrected by `0020`, never by editing `0019`.
- **Data-loss surfaces.** The reconcile pass deletes/relinks override and
  merged rows; a wrong `(author, title)` match could *mis*-link an override
  to the wrong book. Mitigate with exact-normalized matching only (reuse
  `normalize_title`/`normalize_author`, which is deliberately exact — a miss
  costs a manual relink, a false positive corrupts a book) and surface
  unmatched rows as "unlinked" rather than deleting them.
- **What's irreversible once data accumulates.** Once F3.2 ratings/journals
  and F1 user data are keyed on `identity_uuid`, the anchor *definition*
  (precedence rules, the unstable-id detector) is frozen — changing it later
  re-keys live user data with no SQL-computable mapping. Lock decision 2
  before that data exists.
- **Rollback for Option A specifically.** Until a later migration retires the
  legacy `books.uuid` key, the additive columns can be ignored and the
  dual-resolver falls back to the legacy key — so a botched rollout degrades
  to today's behavior rather than data loss. This reversibility is the whole
  reason for recommending A.

## Sequencing & dependencies

F2 sits in a tight chain with F1 and F10:

- **F1 (user-data soft-refs) ↔ F2.** Co-dependent but currently *latent*:
  the five user-data tables still key on numeric `book_id`, so today only
  `metadata_overrides` + `merged_uuids` detach on a re-key. F1 must move user
  data to `book_uuid` soft-refs first; only *then* does a stable uuid matter
  for ratings/progress. Land **F1 then F2**, or as a stacked pair sharing
  migrations `0019`/`0020`. F2 without F1 fixes the identity but leaves user
  data cascade-deleting; F1 without F2 makes the soft-ref contract a lie on
  path change. Both are needed before F3.2.
- **F10 (override GC / reconcile).** The reconcile/relink pass this doc adds
  to `reindex` is the same machinery F10 needs to GC orphaned overrides and
  cover files. Build the reconcile pass once, shared.
- **F3.2 ratings & journaling** is the downstream feature blocked on this —
  the roadmap lists the uuid fix as "must land before this feature ships."
- **Audiobook anchor (decision 3)** can land in the same change or a fast
  follow; it is independent of the ebook `dc:identifier` path.

Must land first: F1's table moves (or at least its `0019` migration shape)
so F2 and F1 don't fight over the migration number. Confirm whether they
stack on one migration or two before authoring either.

## Open questions

1. **Hash timing.** Compute `content_hash` at Phase-A stat (re-reads every
   file every scan — costly) or only for New/Changed books in Phase B
   (cheap, but Unchanged books never get a hash until they next change)?
   Leaning Phase-B-only, accepting that the backfill fills the rest once.
2. **Unstable-id heuristic precision.** Is "`urn:uuid:` prefix without a
   stable publisher/ISBN companion identifier" the right detector, or do we
   need a per-publisher allowlist? A false "stable" classification pins
   identity to a per-export random id and defeats the whole fix.
3. **Audiobook anchor exact form** — first-part content hash vs. a manifest
   hash of all parts' (name, size). The latter survives re-encoding a single
   chapter; the former is cheaper.
4. **Reconcile UI surface** — do we build the "unlinked annotations" view now
   (F3.2 mentions it) or stub it to a log line and a count until F3.2's
   detail page exists?
5. **Legacy-key retirement** — do we ever run the `0020` re-key of
   `merged_uuids`/`metadata_overrides` to `identity_uuid`, or keep the
   dual-resolver indefinitely? Cheap to defer; decide when the two-column
   model proves either fine or confusing in practice.

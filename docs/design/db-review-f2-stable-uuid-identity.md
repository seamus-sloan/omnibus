# F2 — Durable book identity: stored UUID + relative-path scan key

Status: Decided — `books.uuid` becomes a durable stored UUID (never
recomputed), and a new `books.scan_key` keyed on the path *relative* to the
library root takes over the Phase-A diff. Supersedes the earlier
anchor-precedence (`dc:identifier` / content-hash) proposal, which is dropped.
See "The chosen design" below. Companion findings: F1 (user-data soft-refs)
and F10 (override GC / reconcile).

This doc records the data-model decision and sketches the migration; DDL is
not to be applied until F1's table moves are sequenced (see "Sequencing").

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

Two facts the original review under-stated. Both are why identity cannot be
*derived* cheaply — and therefore why the chosen design stores it instead:

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
`stable_uuid(library_path_key, &dir)` in `db/src/audiobook/group.rs`). Under
an anchor scheme that would force a separate audiobook rule; under the chosen
design it is a non-issue — audiobooks get the same stored random `uuid` and a
`(library_id, relative_dir)` scan key as ebooks, with no content hash and no
special-casing.

## Decision required — resolved

The four open points below are all settled by the chosen design: identity is a
stored random UUID (no derivation, no anchor), and the only move case that
needs auto-handling — a library-root repoint — is handled by keying the diff on
the *relative* path. The owner notes that drove each call are kept for
rationale.

1. **Two-key split vs. re-key `books.uuid` in place.** *Resolved:* two keys,
   but neither is derived from content. `books.uuid` is the durable identity —
   a stored random UUID, never recomputed (existing values reused, so nothing
   re-keys). A separate `books.scan_key` does the Phase-A diff.
   - *From owner: `books.uuid` should be the durable identity. The UUID, however,
     does not need to be hashed against the file location. Instead, we should just
     generate our own UUID and not hash against any data. There are multiple
     scenarios where we could end up with two books that have the same title & author
     which would create duplicate UUIDs. If we just generate a fresh UUID, then
     there's no chance for it to be a duplicate.*
2. **Anchor precedence and the "unstable id" detector.** *Resolved: dropped.*
   No `dc:identifier`, no content hash, no unstable-id heuristic. Identity is
   minted, not derived, so there is nothing to rank or reject.
   - *From owner: Why do we even need an anchor for this? As far as we are
     concerned, we just need to have a UUID for the audiobook -- which can just
     be randomly generated just like suggested for the ebook.*
3. **Audiobook anchor.** *Resolved: none needed.* Audiobooks use the identical
   stored-UUID + relative-path scheme as ebooks.
   - *From owner: Again, we don't really need an anchor, do we?*
4. **Reconcile / relink policy.** *Resolved:* a root move preserves `uuid`
   automatically (relative-path diff), so nothing detaches. A within-library
   single-file move is not auto-matched — the moved file rescans as a New row,
   the original is retained as a fileless "ghost", and the user merges the two
   via the existing `0016` merge machinery; relink is an `UPDATE` of the
   soft-ref `book_uuid` (not an FK cascade — a cascade would *delete* children).
   - *From owner: If an anchor is moved for whatever reason, then it should
     just relink with the new UUID in a cascade. Is that a bad idea?*

## The chosen design — durable stored UUID + relative-path scan key

The identity problem splits into two jobs that today are wrongly served by
one value:

1. **Identity** — "which logical book is this row?" Must be stable forever.
2. **Diffing** — "does this file on disk correspond to an existing row?" The
   Phase-A scan-time join.

Identity does *not* need to be derivable from anything — it only needs to be
**assigned once and persisted**. So drop the whole anchor-precedence scheme
(`dc:identifier` / content hash). It was only ever needed because `books.uuid`
doubled as the diff key and therefore had to be recomputable. Separate the two
jobs and the derivation disappears.

### How it works

**`books.uuid` becomes the durable identity, and is never recomputed.** It is
minted once — a fresh random UUID (v4) at insert — and stored. Existing rows
keep their current `uuid` values verbatim (they are already unique), so nothing
re-keys: every cover file, `merged_uuids` row, `metadata_override`, and
`/api/covers/{uuid}` URL stays valid. There is no `dc:identifier` lookup, no
SHA-256, no opening files at boot. A random UUID also cannot collide — two
distinct books that happen to share a title & author (or even identical bytes)
each get their own id, which a content-derived scheme could not guarantee.

**A new `books.scan_key` takes over the Phase-A diff**, recomputed from the
filesystem on every scan. Crucially it is keyed on **`(library_id,
relative_path)`** — the path *relative to the scan root* — **not** today's
`hash(absolute_root_path + relative)`. (`scan_roots` is the table, renamed from
`libraries` in `0019`; `books.library_id` kept its column name.) This makes a
**scan-root repoint transparent**: pointing a library at a new path leaves every
file's relative path unchanged, so the diff matches every row, preserves every
`uuid`, and detaches nothing — no ghosts, no manual work.

**Two settings-path changes are required for that to hold**, because today a
repoint is destructive: `upsert_scan_root` keys on `path`, so a new path
*inserts a new `scan_roots` row* (new `library_id`), and
`prune_orphan_libraries` then *cascade-deletes* the old root's books. So —

1. **A repoint must update the existing `scan_roots` row's `path` in place**
   (the same settings slot keeps its row and its `id`), so `library_id` stays
   stable and `(library_id, relative_path)` still matches across the move.
2. **`prune_orphan_libraries` must stop cascade-deleting books.** A removed scan
   root leaves its books behind as fileless ghosts (below), never a hard delete.
   This is the never-prune safety net: even if the relative-path match ever
   misses, nothing is lost — the rows wait for the user to relink.

**Genuine orphans and within-root single-file moves** are *not* auto-matched —
no fuzzy `(title, author)` guessing. A moved/renamed file (different relative
path under the same root) rescans as a New `books` row with its own file; the
original becomes a **fileless "ghost" row that is retained, not deleted**, so it
keeps its `uuid` and all soft-referenced user data. The user relinks by merging
the new row into the ghost via the existing `0016` format-merge machinery, which
restores every association. A GC routine reaps ghosts that stay fileless past a
window (or immediately after a successful merge), and the orphaned `book_files`
row is cleaned up on the next scan.

**Audiobooks use the identical scheme** — a stored random `uuid` plus a
`(library_id, relative_dir)` scan key. No `dc:identifier`, no content hash, no
special-casing.

### Migration shape

Additive and non-destructive. `ALTER TABLE books ADD COLUMN scan_key TEXT`; a
boot backfill computes `scan_key` for existing rows purely from their stored
`library`/`path` (no filesystem reads, no file opens), mirroring
`backfill_norm_columns` — idempotent, a no-op once caught up. `books.uuid` is
**untouched**: it is already unique and simply stops being recomputed. The sync
writers switch to computing/matching on `scan_key` and, on a match, **preserve
the existing `uuid`** instead of delete-and-reinsert. Forward-only, complies
with rule 06.

### Blast radius

Small and contained. One new column + one boot backfill + a change to the
diff/sync writers to key on `scan_key` and preserve `uuid` on match + the
"retain fileless ghost + GC" policy. The identity value never changes, so there
is **no cover-file rename, no `merged_uuids` re-point, no broken `/api/covers`
URL, and no boot-time file opening** — the costs that made an in-place re-key
look expensive. `resolve_book_id_by_uuid` stays simple (`books.uuid` is already
canonical; the `merged_uuids` fallback remains for merged formats).

### Pros / cons

Pros. Lowest realistic risk; identity is stable by construction (stored, never
derived); the motivating root-move case is fixed for free by relative-path
keying; no destructive migration, no file opens at boot, no collision surface.
Cons. Within-library single-file moves require a manual merge (acceptable: they
are rare and user-initiated, and the merge UI already exists); a "ghost row +
GC" lifecycle must be added; random UUIDs are install-local, so the same file on
two installs gets different ids — cross-install dedup, if ever wanted, belongs
to a sync-layer content fingerprint, not this key.

## Why this over the alternatives

Two paths were considered and rejected in favour of the design above.

- **Derive identity from content (anchor precedence: `dc:identifier` →
  SHA-256 → scan key).** Rejected. It opens every file at boot to backfill,
  needs an unstable-id heuristic that can silently pin identity to a per-export
  random id, and a content/`dc:identifier` collision (identical bytes, or a
  shared bogus id) would reject a legitimately distinct book under the unique
  index. A stored random UUID has none of these failure modes — identity is
  stable because it is *persisted*, not because it is *recomputable*.

- **Re-key `books.uuid` to a new derived value in place.** Rejected for the
  same reason it looked expensive: a destructive table-recreate that renames
  every cover file and re-points every `merged_uuids` row at cutover. The
  chosen design keeps the *existing* `uuid` values and merely stops recomputing
  them, so that entire blast radius evaporates.

Cost-of-delay. This must land (with F1) before F3.2 ratings/journals
accumulate, because once live user data is keyed on `uuid` the lifecycle
hardens. The chosen design is the cheapest way to get there — additive column,
no file opens, no re-key — so there is no reason to defer it behind a heavier
scheme.

## Migration plan (SKETCH — not to be applied yet)

The migration number (`NNNN`) is allocated at implementation time — the next
free number after the highest applied at that point. Forward-only, append-only
per rule 06. `books.uuid` is **not touched** — it is already unique and simply
stops being recomputed.

```sql
-- NNNN_book_scan_key.sql  (SKETCH — durable-uuid + relative-path scan key)
ALTER TABLE books ADD COLUMN scan_key TEXT;   -- nullable; backfilled at boot

-- A scan key locates exactly one row per library on the diff path.
CREATE UNIQUE INDEX idx_books_scan_key
    ON books(library_id, scan_key) WHERE scan_key IS NOT NULL;
```

The `.sql` is additive only. `scan_key` is computed purely from values already
in the row (`library_id` + the stored relative `path`) — **no filesystem reads,
no file opens** — so the boot backfill mirrors `normalize::backfill_norm_columns`
exactly: idempotent, a no-op once caught up:

```rust
// db/src/normalize.rs (or a new identity.rs) — SKETCH
// Idempotent: only touches rows WHERE scan_key IS NULL. Pure DB work —
// safe to run against in-memory test DBs (no filesystem dependency).
pub async fn backfill_scan_keys(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    // SELECT id, library_id, path FROM books WHERE scan_key IS NULL
    // for each: scan_key = scan_key_for(library_id, relative_path(path))
    //           UPDATE books SET scan_key = ? WHERE id = ?
    Ok(())
}
```

No cover renames, no `merged_uuids` re-point, no boot-time file opening — the
durable `uuid` never changes, so none of those keys move. The only new runtime
behaviour beyond the column is in the sync writers (match on `scan_key`,
preserve `uuid` on match) and the **retain-fileless-ghost + GC** lifecycle for
within-library single-file moves.

## Affected code

From the scout spec (current symbols, not line anchors — anchors rot):

- `db/src/helpers.rs` — `stable_uuid` is repurposed as the scan-key derivation
  (rename to `scan_key`/`scan_key_for`) and keyed on `(library_id,
  relative_path)`, dropping the absolute `library_path` from the hash so a root
  repoint does not change it. No identity-derivation helper is added — identity
  is a freshly generated UUID (`uuid::Uuid::new_v4`), not derived.
- `db/src/ebook/parse.rs` — no change for identity. (`dc:identifier` parsing is
  irrelevant to the key now; leave it as bibliographic metadata.)
- `db/src/ebook/stat.rs` — `StatEntry.uuid` becomes the scan key (relative
  path), used only for diffing; it is no longer the book's identity.
- `db/src/sync/books.rs` — `insert_book_row`, `sync_changed`,
  `try_attach_new_ebook`, `attach_ebook_file`: mint `uuid` once at insert and
  **preserve it on a `scan_key` match** instead of delete-and-reinsert; never
  recompute `uuid`.
- `db/src/indexer.rs` — `diff_library` matches on `scan_key`; the Removed
  branch **retains** the now-fileless `books` row (ghost) rather than deleting,
  and a GC pass reaps ghosts past a window / after merge.
- `db/src/settings.rs` — two changes. (1) A repoint must **update the existing
  `scan_roots` row's `path` in place** (per settings slot) instead of
  `upsert_scan_root` inserting a new row, so `library_id` stays stable and the
  relative-path scan key matches across the move. (2) `prune_orphan_libraries`
  must **stop cascade-deleting books** — a removed root leaves its books as
  ghosts, never a hard delete.
- `db/src/books/get.rs` — `resolve_book_id_by_uuid` is unchanged in shape:
  `books.uuid` is already canonical; the `merged_uuids` fallback stays for
  merged formats.
- `db/src/books/projection.rs` — `row_to_ebook` may now surface the real OPF
  `unique_identifier` (bibliographic) instead of echoing `books.uuid`, since
  `uuid` is no longer pretending to be the OPF id. Optional, not required.
- `db/src/audiobook/group.rs` — same scheme as ebooks: stored `uuid` +
  `(library_id, relative_dir)` scan key. No content hash.
- `db/migrations/NNNN_book_scan_key.sql` — additive `scan_key` column + partial
  unique index on `(library_id, scan_key)`.
- `db/src/normalize.rs` (or new `identity.rs`) — `backfill_scan_keys` (pure DB,
  no filesystem), wired into `db/src/pool.rs` `init_db`.
- `db/src/test_support.rs` — seed factories updated for `scan_key`.
- `db/src/helpers/tests.rs` — `scan_key` derivation tests (root repoint leaves
  the key unchanged; relative move changes it).

## Test plan

Per rule 03: sibling `<mod>/tests.rs`, all against `sqlite::memory:`. The
**acceptance test that must fail on the old schema** is the root-repoint
survival test below — on today's code the uuid moves, so it fails until the
relative-path scan key lands.

- `db/src/helpers/tests.rs` — `scan_key`: a library-root repoint with the same
  relative path yields the SAME key (happy); a different relative path yields a
  different key; the same relative path in two different libraries yields
  different keys (the `library_id` scoping). UUID minting uses `new_v4`, so no
  determinism/collision assertion is needed — assert two fresh mints differ.
- `db/src/sync/tests.rs` — **acceptance:** seed a book, reindex under a *new*
  library root, assert the SAME `books.uuid` survives and that
  `metadata_overrides` + `merged_uuids` + the cover URL still resolve unchanged.
  Assert `insert_book_row` / `sync_changed` / the attach paths preserve `uuid`
  on a `scan_key` match rather than minting a new one.
- Ghost-lifecycle test — move a single file to a new relative path under the
  same root, reindex, assert (a) a New `books` row exists for the moved file,
  (b) the original is retained as a fileless ghost holding its `uuid` + soft-ref
  user data, (c) a manual merge restores the associations, (d) the GC pass reaps
  the leftover ghost.
- `db/src/books/tests.rs` — `resolve_book_id_by_uuid` resolves by `books.uuid`
  and by `merged_uuids` (one happy per branch) — unchanged shape.
- Backfill test — seed a row with NULL `scan_key` (simulate pre-migration), run
  `backfill_scan_keys` twice, assert it populates once and is a no-op the second
  time (idempotent, like the `backfill_norm_columns` tests). No filesystem
  needed — pure DB.
- Audiobook test — `books.uuid` stable across a root repoint for an MP3 group
  (`db/src/audiobook` tests), via the same relative-dir scan key.
- Migration test — assert the new migration adds `scan_key` + the partial
  unique index and that a duplicate `(library_id, scan_key)` is rejected. The
  `MIGRATOR` runs it in every existing memory-DB test, so the migration is
  already exercised.

## Risks & rollback

- **Forward-only / fix-forward.** No down-migrations (rule 06). A bug in the
  new migration is corrected by a later one, never by editing it once applied.
- **No auto-relink, so no mislink surface.** The design deliberately avoids
  fuzzy `(author, title)` matching: a root move preserves `uuid` exactly, and a
  within-library move is reconciled by an explicit user merge. The cost of a
  missed match is a manual merge, never a corrupted book.
- **Ghost accumulation.** The retain-fileless-ghost policy means a deleted (not
  moved) book lingers as a ghost until GC. Risk: clutter, or a user merging
  into a ghost they meant to discard. Mitigate with a clear "missing file" badge
  in the UI and a GC window short enough to reap true deletions but long enough
  to bridge a move-then-rescan.
- **What's irreversible once data accumulates.** Once F3.2 ratings/journals and
  F1 user data are keyed on `books.uuid`, the value is frozen — but since it is
  a stored random UUID with no derivation, there is nothing whose *definition*
  can drift. The only frozen contract is "uuid is preserved across moves," which
  this design upholds.
- **Rollback.** `scan_key` is purely additive; ignoring the column and falling
  back to recomputing the old path-uuid restores today's behaviour. `books.uuid`
  values are never rewritten, so a botched rollout degrades to today's behaviour
  rather than data loss.

## Sequencing & dependencies

F2 sits in a tight chain with F1 and F10:

- **F1 (user-data soft-refs) ↔ F2.** Co-dependent but currently *latent*:
  the five user-data tables still key on numeric `book_id`, so today only
  `metadata_overrides` + `merged_uuids` detach on a re-key. **F2 should land
  first** (or in the same train): until the uuid is stored-and-stable, F1's
  soft-refs would still orphan on a repoint and could not auto-relink, because
  the uuid itself would move. With F2 in place the soft ref is durable
  immediately. F2 without F1 fixes the identity but leaves user data
  cascade-deleting; F1 without F2 makes the soft-ref contract a lie on a path
  change. Recommended order: **F2 → F1 → F3.2** (matching the F1 doc), or a
  stacked pair sharing two adjacent migration numbers with F2's the lower —
  F1's migration shape must be known so the two don't collide on a number.
  Both are needed before F3.2.
- **F10 (missing-files GC).** F2's ghosting (a removed file retains its `books`
  row fileless) is what F10's GC reaps: it purges long-missing, user-data-free
  ghost rows. Shipped — see
  [db-review-f10-override-gc.md](db-review-f10-override-gc.md).
- **Known limitation — path-based identity smears overrides (revisit here).**
  A book is identified by `(library slot, relative scan_key)`, not file content.
  Repointing a slot to a directory whose file sits at the *same relative path*
  adopts the prior book's identity — so a `metadata_overrides` row (and user
  data) persists onto a physically different file rather than the two being
  recognized as distinct books. Surfaced by F10's
  `identical_relative_path_in_repointed_directory_*` test. The fix, if wanted,
  is a content-based anchor (the dropped `dc:identifier`/hash proposal) and
  belongs to this finding, not F10.
- **F3.2 ratings & journaling** is the downstream feature blocked on this —
  the roadmap lists the uuid fix as "must land before this feature ships."
- **Audiobooks** need no separate work — they share the ebook scheme verbatim,
  so they land in the same change.

Coordination note: F1's migration *shape* must be known before authoring
either migration so F2 and F1 don't collide on a number — but F2 *lands*
first (lower migration number) per the order above. Confirm whether they stack
on one migration or two before authoring either.

## Open questions

1. **Scan-root reuse on repoint (decided — needs an implementation note).**
   *Decided:* a repoint updates the existing `scan_roots` row's `path` in place
   so `library_id` is stable and `(library_id, relative_path)` matches across the
   move; `prune_orphan_libraries` no longer cascade-deletes (never-prune safety
   net). Open sub-point: how does the settings layer map a changed path back to
   "the same slot's `scan_roots` row" to update — by settings key (ebook /
   audiobook slot → its current root), since `upsert_scan_root` currently keys on
   `path` and has no slot identity. If multiple roots-per-slot ever land, revisit
   (relative-path-only keys would then risk cross-root collisions).
2. **GC window for ghosts.** How long does a fileless `books` row linger before
   GC reaps it? Long enough to bridge a move-then-rescan and let the user merge;
   short enough that a genuine deletion doesn't clutter the library. A fixed
   window, or "reap on next full scan that still doesn't see the file"?
3. **Ghost UX.** How is a fileless ghost surfaced — a "missing file" badge,
   hidden by default with a toggle, or only shown in a merge picker? F3.2's
   detail page may want to render it; until then, a badge + log line may suffice.
4. **Within-library move detection.** Do we want *any* cheap auto-match beyond
   relative path (e.g. same `(filename, size)` at a new relative path → preserve
   uuid without a manual merge), or is manual merge the deliberate, only path?
   Leaning manual-only to avoid a heuristic, but worth confirming against how
   often single-file moves actually happen.

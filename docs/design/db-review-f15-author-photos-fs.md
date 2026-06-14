# F15 — Move author-photo BLOBs to the filesystem

Status: Proposed — deferred from db.md F15, awaiting decision.

This doc frames the one data-model decision the operator must settle before any
schema or code is written: **what happens to the inline image bytes on the
cutover, and what key names the file on disk.** No `.sql` or `.rs` is touched
until that decision lands. The migration DDL and code shapes below are
*sketches* to make the trade-offs concrete, not work to apply.

---

## Problem

`author_photos.bytes` stores the full image — manual admin upload or a fetched
Open Library photo — as a `BLOB` inline in the primary SQLite database
(`0008_author_photos.sql:16`). Book covers were deliberately moved the other
way: they live on disk under `covers_dir()` (`db/src/covers.rs`, `covers_dir`),
and `0003_drop_legacy_tables.sql` dropped the legacy `book_covers` BLOB table.
Author photos diverge from that established pattern for no stated reason.

Inline image BLOBs are costly in exactly the ways the covers move was meant to
avoid:

- They inflate the main DB file and are copied wholesale by `VACUUM` and any
  backup/replication of the SQLite file (assumption A1: single-instance,
  file-backed deployments — the DB file *is* the backup unit).
- They sit on the same pages the hot taxonomy/browse queries scan. Two hot paths
  probe `author_photos` **per author row**:
  - `list_authors` inlines `EXISTS(... WHERE source IN ('manual','openlibrary')
    AND bytes IS NOT NULL)` as a `has_photo` projection column
    (`db/src/browse.rs:76-81`) — one probe per author in the taxonomy grid.
  - `author_has_usable_photo` runs the same `EXISTS` per author
    (`db/src/discovery/authors.rs:170-182`), called from `get_author`.

  Both key the "has a real photo" decision on `bytes IS NOT NULL` to distinguish
  a usable photo from a `'letter'` negative-cache marker. With a 10 MiB upload
  cap (`server/src/backend/author_photos.rs:112`, referenced by the comment at
  `db/src/ebook/accent.rs:9`), a library of a few hundred authors can carry tens
  of MiB of image data on pages the author grid walks every render.

**Who it bites.** The author-discovery surface — F1.11 Author profiles and F1.12
Browse authors & series (roadmap `0-0-summary.md` Phase 1) — is precisely the
feature that runs these per-row probes at scale. The grid was the motivation for
`has_photo` being a projection column in the first place; leaving image bytes on
those pages is a self-inflicted N-author scan tax on the page that reads the flag
hardest.

The CRUD that has to move is small and already located:
`get_author_photo` SELECTs `bytes` (`db/src/author_photos_data.rs:64-76`),
`upsert_author_photo` binds `bytes` (`:105-123`), `delete_author_photo`
(`:127-136`). `author_photo_status` (`:82-92`) is already byte-free. Writers:
the OL/letter cascade (`db/src/author_photos/cascade.rs:104-118`), the server
multipart + URL upload handlers (`server/src/backend/author_photos.rs`), and the
RPC URL upload (`frontend/src/rpc.rs`). The serve route streams bytes from
`db::get_author_photo` (`server/src/backend/author_photos.rs:34-45`).

---

## Decision required

Two coupled choices, both forced by SQLite reality (no in-place column drop here;
table-recreate required — see Options) and by the append-only migration rule
([rule 06](../../.claude/rules/06-migrations.md)).

1. **Cutover semantics for the existing bytes — drain or discard?**
   The migration that drops the `bytes` column runs *inside* `init_db` via the
   embedded `MIGRATOR` (`db/src/pool.rs:49-52`), *before* any app code we could
   write to read those bytes. So the table-recreate's `INSERT … SELECT` either
   carries the bytes forward (into a staging table, for a later boot pass to
   drain) or drops them on the floor. Concretely:
   - **(a) Discard** — treat all cached photos as a rebuildable cache; let the OL
     cascade re-resolve `openlibrary` rows on next view. **Manual admin uploads
     are permanently lost** (OL has no copy to re-fetch).
   - **(b) Drain** — a transitional staging table preserves the bytes through the
     migration, and a one-time idempotent boot pass writes every BLOB to the new
     photos dir before clearing staging. Preserves manual uploads; costs one
     staging table and one boot backfill.

2. **Filename key on disk — `author_id` or a per-row token?**
   - `author_id` (e.g. `<photos_dir>/<author_id>.<ext>`) is stable, simple, and
     mirrors covers' `<uuid>.<ext>`. Risk: SQLite reuses rowids, so a
     *deleted-then-re-added* author can inherit an old id and a stale file —
     unless `upsert`/`delete` always rewrite/unlink the file (which we control).
   - A per-row random token stored in the row (`photo_token TEXT`) eliminates id
     reuse entirely at the cost of an extra column, a `find_*_file` that must read
     the token first, and orphan files when a row is replaced without unlinking.

These two are the heart of the doc. Everything else (FS helpers, predicate swap,
docs) is mechanical and identical across options.

---

## Options

All options share the migration's structural core: SQLite cannot cleanly
`DROP COLUMN bytes` here. The column participates in the table-level `CHECK`
(`0008:23-26` ties `bytes`/`mime` presence to `source`), and SQLite refuses to
drop a column referenced by a CHECK (and pre-3.35 cannot `DROP COLUMN` at all).
The fix is a forward-only **table-recreate**: build `author_photos_new` keyed on
`mime` instead of `bytes`, `INSERT … SELECT`, `DROP`, `RENAME`. They differ only
in what the SELECT carries and what runs at boot.

### Option A — Discard bytes, key by `author_id` (cache-rebuild)

**How it works.** The recreate's `INSERT … SELECT` omits `bytes` entirely. On
first boot after upgrade, `openlibrary` rows still exist (source/url/mime kept)
but no file is on disk, so `get_author_photo` returns `None` → serve 404 →
background `ResolveAuthorPhoto` re-fetches from OL and writes the file. `'letter'`
markers survive untouched (they never had bytes). Manual uploads become broken
`manual` rows pointing at a missing file → 404 forever until re-uploaded.

**Migration shape.** One file, table-recreate, no staging table, no boot pass.
The new `CHECK` keys on `mime` (`source <> 'letter'` ⟺ `mime IS NOT NULL`).

**Blast radius.** Migration + FS helpers + CRUD rewrite + two predicate swaps +
docs. No `pool.rs` boot change. Smallest diff.

**Pros.** Simplest; lowest-risk code; no transitional table to clean up later; no
boot-time I/O sweep. Honest about photos being a cache for the OL-sourced
majority.
**Cons.** Silently discards admin-uploaded photos — the one class of photo that
is *not* rebuildable. A library that curated 50 author headshots loses all 50 on
upgrade with no warning unless we surface it in release notes.

### Option B — Drain bytes to disk, key by `author_id` (preserve uploads)

**How it works.** Migration `00NN` adds a transitional staging table
(`author_photo_bytes_staging(author_id, mime, bytes)`) and copies every
non-letter row's bytes into it *before* the recreate drops the column. A new
idempotent boot pass `backfill_author_photos_to_fs` in `init_db` (after
`MIGRATOR.run`, mirroring `normalize::backfill_norm_columns` at `pool.rs:56`)
reads staging in batches, writes each `<author_id>.<ext>` via the FS helper,
deletes the staged row on success, and is a no-op once staging is empty. When
staging drains to zero, the pass is free on every subsequent boot.

**Migration shape.** Same recreate as A, plus a `CREATE TABLE … staging` +
`INSERT INTO staging SELECT author_id, mime, bytes FROM author_photos` *ahead of*
the `DROP`. The staging table is left in place (forward-only); the boot pass
empties it. A later cleanup migration may drop the empty staging table once we're
confident every install has drained.

**Blast radius.** Everything in A plus a `pool.rs` boot pass and its tests. One
extra migration concept (staging) that lingers until a follow-up drop.

**Pros.** Zero data loss — manual uploads and already-fetched OL photos both
survive the move, so the first post-upgrade render is identical to pre-upgrade.
No thundering re-resolve against Open Library on first boot.
**Cons.** More moving parts: a transitional table, a boot pass that does
synchronous `std::fs` writes (must run on `spawn_blocking` like the cover purge,
`pool.rs:66-84`), and an eventual cleanup migration. The staging table holds the
full BLOB set *twice* transiently (original rows + staging) during the single
migration transaction — fine for a personal library, noted for honesty.

### Option C — Per-row token key (orthogonal to A/B on the bytes question)

**How it works.** Instead of `<author_id>.<ext>`, store a random `photo_token`
on the row and name files `<photo_token>.<ext>`. `upsert` generates a fresh token
(invalidating the old file, which it unlinks), `find` reads the token then the
file. Combine with either A or B for the bytes question.

**Migration shape.** Adds a `photo_token TEXT` column to the recreated table.

**Blast radius.** Same as A/B plus a column, token generation on every upsert, and
a token read on every serve (the `EXISTS` probes don't need it — they still key on
`source`).
**Pros.** Immune to rowid reuse; a stale file can never be served as a new
author's photo even if the unlink is skipped.
**Cons.** Extra column and indirection for a problem we already control:
`upsert`/`delete` own the file lifecycle, so an `author_id`-keyed file is
overwritten on re-upload and unlinked on delete. The reuse window only opens if a
write path forgets to touch the file — which is exactly what tests pin down.
`sha2` is already a workspace dep if we want content-addressed tokens, but that
re-introduces dedup/refcount questions covers never needed.

---

## Recommendation

**Option B — drain to disk, keyed by `author_id`.** Rationale:

- **The asymmetry decides it.** OL-sourced photos are rebuildable; manual uploads
  are not. Option A's "it's just a cache" framing is true for the majority and
  false for the one class a human curated by hand. Once a library accumulates
  manual uploads, discarding them on a *storage-layout* migration is a data-loss
  surprise with no user-facing cause — the worst kind. The cost of avoiding it is
  one idempotent boot pass we already have a worked model for
  (`backfill_norm_columns`, `purge_legacy_covers_once`).
- **Reject the token (Option C).** The rowid-reuse risk is real but fully closed
  by making `upsert_author_photo` always (re)write the file and
  `delete_author_photo` / `delete_author` always unlink it — behavior the test
  plan pins. A token buys insurance against a bug the tests already forbid, at the
  cost of a column and a serve-path read on the hottest image route. Mirror
  covers: `author_id` is to photos what `uuid` is to covers.
- **Reversibility / cost of delay.** The bytes-vs-FS choice is *reversible while
  the data is small and few installs exist* — today. Once real libraries
  accumulate manual uploads, Option A becomes irreversibly lossy and Option B's
  drain becomes the only safe path anyway. Deciding now, for B, costs the least;
  deferring raises the floor to B regardless and risks an A-shaped accident in
  the interim. The migration is forward-only either way (rule 06), so there is no
  "undo" — only fix-forward.

If the operator knows no manual uploads exist in any live install, **Option A is
acceptable and strictly simpler** — it collapses to "drop bytes, let OL
re-resolve." That is the single fact that flips the recommendation. The default,
absent that certainty, is B.

---

## Migration plan (SKETCH — not to be applied)

New file `db/migrations/00NN_author_photos_fs.sql` (next zero-padded number after
the current highest; **do not** hardcode `0019` here — renumber at authoring time
per rule 06). Table-recreate with the new `CHECK` keyed on `mime`. The staging
lines are the **Option-B** additions; Option A omits them.

```sql
-- 00NN_author_photos_fs.sql
-- Move author-photo bytes to the filesystem (mirrors covers). Row keeps only
-- (source, url, mime, fetched_at); bytes live at
-- <OMNIBUS_AUTHOR_PHOTOS_DIR>/<author_id>.<ext>.

-- [Option B only] Preserve bytes across the column drop. A boot pass drains
-- this to disk and empties it; a later migration drops the empty table.
CREATE TABLE author_photo_bytes_staging (
    author_id INTEGER PRIMARY KEY,
    mime      TEXT NOT NULL,
    bytes     BLOB NOT NULL
);
INSERT INTO author_photo_bytes_staging (author_id, mime, bytes)
    SELECT author_id, mime, bytes
      FROM author_photos
     WHERE source <> 'letter' AND bytes IS NOT NULL;

-- Recreate without `bytes`; CHECK now keys on mime, not bytes.
CREATE TABLE author_photos_new (
    author_id  INTEGER PRIMARY KEY REFERENCES authors(id) ON DELETE CASCADE,
    source     TEXT NOT NULL CHECK (source IN ('manual','openlibrary','letter')),
    url        TEXT,
    mime       TEXT,
    fetched_at TEXT NOT NULL DEFAULT (datetime('now')),
    -- 'letter' = negative-cache marker, no image; non-letter rows carry a mime
    -- and a file on disk at author_photos_dir()/<author_id>.<ext>.
    CHECK (
        (source =  'letter' AND mime IS NULL)
     OR (source <> 'letter' AND mime IS NOT NULL)
    )
);
INSERT INTO author_photos_new (author_id, source, url, mime, fetched_at)
    SELECT author_id, source, url, mime, fetched_at FROM author_photos;
DROP TABLE author_photos;
ALTER TABLE author_photos_new RENAME TO author_photos;
```

Notes:
- `PRAGMA foreign_keys` is already `ON` per-connection (`pool.rs:37`). The migrator
  runs each migration in its own transaction; SQLite defers FK enforcement to
  commit, and `ON DELETE CASCADE` from `authors` survives the rename. Confirm in
  the migration test (see Test plan) that the FK is intact post-rename — do **not**
  hand-toggle `PRAGMA foreign_keys` inside the migration body.
- **Option-B boot pass.** Add `backfill_author_photos_to_fs(&pool)` called from
  `init_db` after `MIGRATOR.run` and after `backfill_norm_columns` (`pool.rs:56`),
  gated `!is_memory` like the cover purge so the test suite never touches a real
  dir, and run under `tokio::task::spawn_blocking` (`pool.rs:66-84` is the model).
  It reads staging in batches, writes each file via the new FS helper, deletes the
  staged row on success, and returns early when staging is empty or absent.
  Idempotent: a second boot finds an empty table and does nothing.

---

## Affected code

From the F15 scout spec, confirmed against current line numbers:

- **`db/migrations/00NN_author_photos_fs.sql`** — NEW. Table-recreate above.
- **`db/src/author_photos.rs`** (or a new `db/src/author_photos/files.rs`) — NEW
  FS layer mirroring `covers.rs`: `author_photos_dir()` reading
  `OMNIBUS_AUTHOR_PHOTOS_DIR` (default `./author-photos`),
  `write_author_photo_file(author_id, mime, bytes)`,
  `find_author_photo_file(author_id) -> Option<(String, Vec<u8>)>`,
  `delete_author_photo_file(author_id)`. Reuse `covers::ImageFormat` (promote its
  `pub(crate)` visibility crate-wide, or copy the small format table).
- **`db/src/author_photos_data.rs`** — `upsert_author_photo` writes bytes to FS
  then upserts `(source,url,mime,fetched_at)` only; `get_author_photo` reads `mime`
  from the row and bytes from FS via `spawn_blocking` (return `None` on missing
  file, like covers); `delete_author_photo` unlinks then deletes the row;
  `delete_author` (`:156-216`) unlinks the deleted author's file; consider an `Io`
  variant on `AuthorPhotosDataError` (or swallow-to-`None` on the read path like
  covers). Update the module `//!` doc.
- **`db/src/browse.rs:76-81`** — swap the `has_photo` `EXISTS` off `bytes IS NOT
  NULL` to `source IN ('manual','openlibrary')` (a non-letter row now implies a
  file should exist).
- **`db/src/discovery/authors.rs:170-182`** — same predicate swap in
  `author_has_usable_photo`.
- **`db/src/covers.rs`** — `ImageFormat` visibility for reuse (or duplicate).
- **`db/src/pool.rs`** — Option B only: `backfill_author_photos_to_fs` boot pass.
- **`server/src/backend/author_photos.rs`** (serve `:34-45`, uploads) and
  **`frontend/src/rpc.rs`** URL upload — **no signature change**; they call
  `db::get_author_photo` / `db::upsert_author_photo`, which keep their
  `(mime, Vec<u8>)` shapes. The DB layer hides the FS.
- **`db/src/author_photos/cascade.rs:104-118`** — unchanged at the call site; it
  already passes `(url, mime, bytes)` to `upsert_author_photo`.
- **Docs** — `.env.example`, `.claude/rules/01-dev-environment.md` (new
  `OMNIBUS_AUTHOR_PHOTOS_DIR`, mirror `OMNIBUS_COVERS_DIR`),
  `.claude/architecture.md` (new dir), and refresh the `0008` BLOB reference in
  the `db/src/ebook/accent.rs:9` comment + the `0008_author_photos.sql` header
  comment, which still describes `bytes`/`mime` as NULL-for-letter.

---

## Test plan

Per [rule 03](../../.claude/rules/03-unit-testing.md): db tests in the existing
`#[cfg(test)] mod tests` in `db/src/author_photos_data.rs` (`:218+`), against
`init_db("sqlite::memory:")`. Add an `AuthorPhotosTempDir` RAII guard to
`test_support` mirroring `CoversTempDir` (`db/src/test_support.rs:145-184`) that
points `OMNIBUS_AUTHOR_PHOTOS_DIR` at a unique temp dir under a held env lock.

**The acceptance test that must fail on the old schema.** A migration/PRAGMA test
asserting `author_photos` has **no `bytes` column** after the new migration:
`PRAGMA table_info(author_photos)` contains no `bytes` row. This fails today
(column present at `0008:16`) and is the red bar the change turns green.

**Update existing cases** to round-trip bytes through the FS:
- `author_photo_roundtrips_manual_upload` (`:227`) — upsert writes a file; read
  returns it.
- `author_photo_letter_marker_returns_none` (`:247`) — a letter marker writes **no**
  file.
- `author_photo_status_none_when_unset` (`:261`) — unchanged contract.
- `author_photo_upsert_replaces_existing_row` (`:268`) — replacing a row rewrites
  the file (and, with `author_id` keying, the old extension's file is gone or
  overwritten).
- `author_photo_delete_clears_row` (`:293`) — delete unlinks the file.

**New cases:**
- `get_author_photo` returns `None` when the row exists but the FS file was
  removed out-from-under it (mirror `cover_returns_none_when_file_missing`,
  `db/src/covers.rs:264`).
- `delete_author` also unlinks the deleted author's photo file.
- per-variant coverage for any new `Io` variant on `AuthorPhotosDataError`
  (happy + one representative FS-failure), or document the swallow-to-`None`
  choice if no `Io` variant is added.
- **Option B:** `backfill_author_photos_to_fs` drains a seeded staging row to a
  file and empties staging; a second call is a no-op; a missing/empty staging
  table is a no-op.

**Predicate-swap regressions:** `db/src/browse/tests.rs`
(`list_authors_populates_has_photo`, ~`:262`) and `db/src/discovery/tests.rs`
(`get_author_populates_has_photo`, ~`:768`) — manual/letter/none must still flip
`has_photo` correctly now that the `EXISTS` keys on `source`, not `bytes`. They
upsert-then-read, so they pass if upsert writes the file; verify the predicate no
longer mentions `bytes`.

**Server round-trip:** `server/src/backend/author_photos/tests.rs` — 404-when-unset,
upload-then-GET-200, `has_photo` flips after upload — run unchanged to confirm the
FS round-trip survives the HTTP boundary.

---

## Risks & rollback

- **Forward-only, no down-migration** (rule 06). The recreate is irreversible once
  applied; recovery from a bad migration is a *new* `00NN+1` file, never an edit to
  the applied one.
- **Data-loss surface (the whole reason this is a decision).** Option A
  permanently discards manual uploads; Option B preserves them but the drain pass
  does synchronous FS writes that can fail (full disk, perms). The drain must be
  best-effort-but-retried: leave the staged row in place on a write failure so the
  next boot retries (do **not** delete-on-attempt), and log loudly — mirror the
  cover purge's logged-and-swallowed posture but *without* the "give up forever"
  sentinel, because losing a staged upload is not a rebuildable-cache failure.
- **Irreversible once data accumulates.** The moment a live install gathers manual
  uploads, Option A stops being a safe choice retroactively — there is no later
  point at which "discard" becomes free again. This is the cost-of-delay: the
  decision is cheap now and only gets more constrained.
- **rowid reuse (Option-A/B `author_id` keying).** A deleted-then-re-added author
  can reuse an id; the mitigation is that `upsert` always rewrites and `delete`
  always unlinks, pinned by tests. Accepted over Option C's column.
- **Boot-time I/O (Option B).** The drain sweeps every staged BLOB on first boot
  after upgrade; on `spawn_blocking`, gated `!is_memory`, awaited like the cover
  purge so the runtime stays schedulable but boot still blocks until it finishes.

---

## Sequencing & dependencies

- **Reindex durability (F1 "user data cascade-deletes on reindex" / F2
  "path-based `stable_uuid` breaks durability").** These govern how stable author
  identity is across reindex. F15 keys files by `author_id`, which is stable
  across reindex *today* (authors are resolved/deduped, not recreated per scan),
  so F15 does not depend on F1/F2 landing first. But if F1/F2's fix ever changes
  how author rows survive a reindex, the `author_id`-keyed file lifecycle is the
  thing to re-check — note the coupling so the F1/F2 work re-reads this doc. There
  is no ordering constraint; there is a "don't break the other's assumption"
  constraint.
- **`metadata_overrides` FS cutover (F10).** The override-cover path
  (`covers_dir()/override-<uuid>.<ext>`, see `db/src/covers.rs:153`) is the closest
  precedent for "user-curated image on disk, soft-referenced from a row." F15
  should mirror its file-naming and 404-on-miss conventions so the two image FS
  layers stay consistent. No ordering constraint; F15 can land independently.
- **Must land first:** nothing. F15 is self-contained — a migration, a FS layer, a
  CRUD rewrite, and two predicate swaps. It is sequenced *after the decision in
  this doc*, not after any other finding.
- **Co-changes in the same PR:** the migration + FS layer + CRUD + predicate swaps
  + the `OMNIBUS_AUTHOR_PHOTOS_DIR` docs must ship together — the predicate swap is
  only correct once a non-letter row implies a file exists.

---

## Open questions

1. **Manual uploads in the wild?** Does any live install hold admin-uploaded
   author photos? If verifiably none, Option A is acceptable and simpler. This is
   the single fact that flips the recommendation from B to A.
2. **Default dir name.** `./author-photos` (mirrors `./covers`) vs nesting under a
   shared data dir like `OMNIBUS_DATA_DIR/author-photos`. Covers use a flat
   sibling dir; HLS uses `OMNIBUS_DATA_DIR/hls/`. Pick one and document it in
   `.env.example`.
3. **Drop the staging table when?** If Option B ships, when is it safe to add the
   follow-up migration that drops the (now-empty) `author_photo_bytes_staging`
   table? Proposal: one release after the drain ships, once every install has
   booted on the draining version at least once.
4. **`Io` variant vs swallow-to-`None`.** Should FS read failures surface as a
   typed `AuthorPhotosDataError::Io`, or fold into `None` like covers' read path?
   Covers swallow; consistency argues swallow, but a manual-upload 404 from a perms
   error is more confusing than a missing cover. Lean swallow-on-read, surface-on-
   write (upload must not silently 200 if the file didn't land).

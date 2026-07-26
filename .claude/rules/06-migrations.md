# 06 — SQL migrations

Schema lives as numbered SQL files under
[db/migrations/](../../db/migrations/), embedded into the binary via
`sqlx::migrate!("./migrations")` (the `MIGRATOR` in
[db/src/pool.rs](../../db/src/pool.rs)) and run on every pool init in
`omnibus_db::init_db`. Applied versions are recorded in the
`_sqlx_migrations` table. The **same** migrator runs against
`sqlite::memory:` in tests, so a migration that works in the test suite
works in production.

## Authoring a migration

1. **Name it `NNNN_short_description.sql`.** Take the next zero-padded
   number after the highest existing file (latest is
   `0050_reading_progress_client_time.sql`). The number is the version
   `_sqlx_migrations` records; renumbering or renaming an applied file
   breaks the applied-version bookkeeping.
2. **Never edit an applied migration.** Once a file has run anywhere
   (your dev DB, a teammate's, CI, production) it is frozen — `sqlx`
   checksums each migration and a changed file fails startup with a
   checksum mismatch. Always fix forward with a new `NNNN_` file.
3. **Forward-only.** There are no down-migrations; write each change so a
   fresh DB and an upgraded DB converge on the same schema.
4. **New `NOT NULL` columns need a default or a backfill.** For values
   the SQL can't compute, follow the `_norm` pattern: add the column,
   then backfill once at boot from `init_db`.
   `normalize::backfill_norm_columns` (added with migration `0016` to
   fill `title_norm`/`author_norm`) is the model — idempotent, and a
   no-op once caught up.

## Book-identity tables

`books.uuid` is the **durable** book identity (F2, migration `0026`): a
random UUIDv4 minted once at insert and never recomputed. The reindex diff
matches disk-vs-DB on `books.scan_key` (the library-relative path), so a
scan-root repoint preserves every uuid; a removed file **ghosts** its book
(drops `book_files`, retains the `books` row) so the identity — and anything
keyed on it — survives. `stable_uuid(library_path, filename)` is no longer
the identity; it survives only as the `merged_uuids` attach-ledger key.

If the new table references a book, mind `merged_uuids` (cross-format
attach/merge, migration `0016`): resolve uuids through
`resolve_book_id_by_uuid` (which falls back to merged uuids) rather than
a bare `SELECT id FROM books WHERE uuid = ?`. Durable user-data tables
should soft-reference `book_uuid TEXT` (no FK, no cascade), like
`metadata_overrides` — not a cascading `book_id` FK that wipes user data
on reindex (see
[docs/roadmap/3-2-ratings-journaling.md](../../docs/roadmap/3-2-ratings-journaling.md)).

## Testing

- DB tests spin up `init_db("sqlite::memory:")`, which runs every
  migration, so a new migration is already exercised by the existing
  suite. Add a test asserting the new column/table behaves, per
  [03-unit-testing.md](03-unit-testing.md).
- There is **no sqlx offline cache** to regenerate: queries use the
  runtime `sqlx::query(...)` API, not the compile-time `query!` macros,
  so there is no `.sqlx/` directory and no `cargo sqlx prepare` step.

## After adding a migration

A running dev server applies migrations only at startup, so a live
`dx serve` won't pick up a new file until it restarts. Run
**`just dev-bounce`** to restart it cleanly (see
[01-dev-environment.md](01-dev-environment.md) and the `justfile`).

> [docs/roadmap/0-2-migrations.md](../../docs/roadmap/0-2-migrations.md)
> is a planning doc about migration *strategy*, not a how-to — this rule
> is the authoring recipe.

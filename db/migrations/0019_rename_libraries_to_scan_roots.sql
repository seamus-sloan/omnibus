-- F3: Rename the physical scan-root registry `libraries` -> `scan_roots`,
-- freeing the name `libraries` for the F3.1 user-facing saved-filter concept.
-- Modern SQLite (>= 3.25, with the default legacy_alter_table off) auto-rewrites
-- the `REFERENCES libraries(id)` clause in `books` to point at the renamed table,
-- so the FK and its ON DELETE CASCADE survive without a table recreate. The
-- runtime SQLite is the environment's (the Nix shells provide pkgs.sqlite via
-- libsqlite3-sys), and the fk_cascade_survives_libraries_rename_to_scan_roots
-- test enforces the behavior we rely on.
-- `books.library_id` keeps its column name (renaming it would force a full
-- books-table recreate for purely cosmetic gain).
ALTER TABLE libraries RENAME TO scan_roots;

-- SQLite has no RENAME INDEX, so drop and recreate the FK index.
DROP INDEX IF EXISTS idx_books_library_id;
CREATE INDEX idx_books_scan_root_id ON books(library_id);

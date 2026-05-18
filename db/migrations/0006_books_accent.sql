-- F1.7 Atrium design system — per-book accent color extracted from the
-- cover during indexing. Stored opaque (a CSS color value, today an
-- `oklch(L C H)` string) so future re-encoding doesn't require a schema
-- change. Nullable: NULL means "no cover or extraction failed" and the
-- frontend falls back to the theme default accent.

ALTER TABLE books ADD COLUMN accent_color TEXT;

-- Partial index supports a future "backfill missing accents" worker job
-- (rows with NULL stay rare after the first reindex, so the index is small).
CREATE INDEX IF NOT EXISTS idx_books_accent_null
    ON books(id) WHERE accent_color IS NULL;

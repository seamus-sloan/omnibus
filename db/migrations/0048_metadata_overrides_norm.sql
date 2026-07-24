-- Effective (override) match keys for Physical Check-In.
--
-- Check-In's fuzzy rung (`find_book_by_norm`) matches a scanned book to a
-- library book on normalized (title, author). It reads `books.title_norm` /
-- `books.author_norm`, which reflect *scanned* metadata — so a user's
-- title/author edit (which lives only in `metadata_overrides`, a read-time
-- overlay) never affected matching, and a renamed book still failed to match.
--
-- These columns carry the normalized *effective* title/author so the resolver
-- can COALESCE the override key over the scanned key. NULL when the override
-- doesn't set that field (fall back to the scanned norm). Populated on override
-- save (`upsert_overrides_row`) and backfilled once at boot
-- (`backfill_override_norm_columns`) for rows written before this migration.
--
-- Orthogonal to the indexer's atomic wipe-and-rewrite, like the rest of this
-- table — a reindex re-derives `books.*_norm` from scanned metadata but never
-- touches these override-derived keys.
ALTER TABLE metadata_overrides ADD COLUMN title_norm TEXT;
ALTER TABLE metadata_overrides ADD COLUMN author_norm TEXT;

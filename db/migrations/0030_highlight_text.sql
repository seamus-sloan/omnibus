-- F2.4b Store the highlighted text alongside each highlight.
--
-- The CFI range locates the span in the book's DOM, but resolving it back to
-- the underlying prose requires the rendered epub.js document. Persisting the
-- selected text lets the highlights drawer and quote cards render the passage
-- without re-opening the book. Nullable + no backfill: highlights created
-- before this migration simply have no stored text.
ALTER TABLE highlights ADD COLUMN text TEXT;

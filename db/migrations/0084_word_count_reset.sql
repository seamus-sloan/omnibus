-- Re-derive `books.word_count` now that the estimate skips navigation
-- documents and `<script>` / `<style>` bodies.
--
-- `ebook::estimate_word_count` used to walk every spine item and count every
-- whitespace token `strip_tags` left behind. That charged a book once more for
-- every chapter title its table of contents lists — an error that grows with
-- the book, which is the worst shape an estimate's error can take — and counted
-- a stylesheet's selectors and a script's identifiers as prose. Both are fixed,
-- which leaves every stored count derived by the old walk stale.
--
-- Nulling a count is how a row is handed back to
-- `indexer::backfill_word_counts` (it only picks rows where `word_count IS
-- NULL`), which re-opens the book's EPUB and recomputes from the same spine.
--
-- Which rows to reset is the whole question, and rule 06 gives two constraints.
--
--   * *Reset only what actually changes.* Unlike `0070`'s ampersand test, no
--     SQL predicate can tell which books carry a nav document or an inline
--     `<style>` — that fact lives inside the archive, not in a column. What SQL
--     *can* narrow is the derivation: only an EPUB-backed row ever received a
--     word count in the first place (the walk needs a spine), so a comic or an
--     audiobook is untouched here and its NULL stays a NULL.
--   * *Never null a column the backfill cannot re-derive.* The backfill's work
--     set is `books` joined to `scan_roots` on `library_id` **at the configured
--     `ebook_library_path`** and to an EPUB `book_files` row, so a row failing
--     either join would be nulled and never refilled. Both joins are repeated
--     here as EXISTS guards.
--
--     The `book_files` one protects a **ghosted** book: removing a file drops
--     its `book_files` rows and keeps the `books` row, so a ghost has no EPUB
--     to re-read and keeps the count it already has.
--
--     The `scan_roots` one has to carry the *path* predicate, not just the
--     foreign key. `books.library_id` is `NOT NULL REFERENCES scan_roots(id)`
--     and foreign keys are enforced, so an EXISTS on the id alone is true for
--     every surviving row and excludes nothing. What it must exclude is a root
--     that still owns books but is no longer the configured one —
--     `settings::prune_orphan_libraries` deliberately keeps such a root, and
--     `backfill_word_counts` is only ever posted with the configured path, so
--     those books would be nulled with nothing left to refill them.
--
-- Residual, and it is a real one: the backfill runs per library and only when
-- posted. `server::main::kick_recovery_scans` now posts it at boot for a
-- library that isn't due a scan (a scan posts it itself, on success), so an
-- upgrade heals on the next start rather than waiting for a scan interval to
-- elapse. Until it runs, the affected books resolve no length on the estimate
-- rung: they contribute nothing to the Pages read total and file under
-- "Unknown" in the length distribution — visibly absent rather than quietly
-- wrong, which is the right way round.
--
-- A row the backfill re-opens but cannot estimate (the file has since moved,
-- the archive is unreadable) is left NULL and retried on the next scan. That is
-- the backfill's existing contract, not new here.

UPDATE books
   SET word_count = NULL
 WHERE word_count IS NOT NULL
   AND EXISTS (
        SELECT 1 FROM book_files bf
         WHERE bf.book_id = books.id
           AND bf.format = 'EPUB' COLLATE NOCASE
       )
   AND EXISTS (
        SELECT 1 FROM scan_roots l
          JOIN settings s ON s.key = 'ebook_library_path' AND s.value = l.path
         WHERE l.id = books.library_id
       );

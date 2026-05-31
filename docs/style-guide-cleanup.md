# Style guide cleanup backlog

Files ordered worst-to-least offending against the rules in
[.claude/rules/05-rust-style.md](../.claude/rules/05-rust-style.md).
A future agent session should work top-down.

| Rank | File | Score | Top violations | Suggested action |
|---|---|---|---|---|
| 1 | db/src/sync.rs | 26 | `sync_books` 210 lines (db/src/sync.rs:60); `insert_author_links` 102 lines; file 1721 lines; 6 section-marker dividers (`// --- Removed`, `// --- Changed`, …); returns raw `sqlx::Error` across module boundary (line 64) | Split into `sync/{mod,books,authors,backfill}.rs`; extract `sync_removed`/`sync_changed`/`sync_new`/`backfill_creator_ids_for_library` from `sync_books`; wrap `sqlx::Error` in module-local `thiserror` enum; move inline tests to `sync/tests.rs` |
| 2 | db/src/discovery.rs | 24 | Module doc 17 lines with roadmap refs + issue tracker (line 1–16); `get_author` rustdoc 22 lines (line 44–65); file 1443 lines; missing happy-path-only test coverage for pub fns returning sqlx::Error | Trim module doc to ≤5 lines, move roadmap context to PR; consolidate `get_author`/`get_series`/`get_tag_cloud` rustdoc to 3–4 lines; wrap sqlx::Error in module-local enum; add error-path tests for each pub fn |
| 3 | db/src/palette.rs | 22 | File 1400 lines; single 26-line fn (search_palette); no module doc; missing /// on pub const MAX_DISCOVERY_BOOKS pattern duplication (similar to discovery.rs) | Add brief module doc; split search_palette helpers into subscope fns; check for overlap with discovery.rs const/patterns and consolidate to shared/ if appropriate |
| 4 | db/src/worker.rs | 21 | File 1296 lines; lack of small focused impl methods per style guide Model reference; error handling sprawl across async task queue | Audit for functions crossing 80 lines; split Worker impl into task_queue, execution, error_handling subscopes |
| 5 | frontend/src/data.rs | 20 | File 1188 lines; 45+ pub fns with mixed doc coverage; no /// on majority of small fetch wrappers; anyhow-based errors without context | Add one-line /// to every pub fn; consolidate fetch-wrapper helpers into named groups; wrap anyhow errors with contextual messages |
| 6 | frontend/src/styles.rs | 18 | File 1124 lines; pure CSS constant string (non-Rust); minimal structure; doc comment bloat (3 lines for a const string) | Move or consider: is this the right place? If kept, trim doc to one-liner |
| 7 | shared/src/lib.rs | 17 | File 881 lines; re-exports + type defs + derives without module structure; missing pub struct/enum doc comments; serde derives without rustdoc | Add /// to each pub struct/enum; consider splitting types into submodules (auth_types.rs, book_types.rs, etc.) if file keeps growing |
| 8 | db/src/author_photos.rs | 16 | File 990 lines; no module doc; multiple pub async fns with sparse /// coverage; inline test block 479 lines (should move to author_photos/tests.rs) | Add brief module doc; add /// to pub fns; promote tests to sibling file; check error handling (raw sqlx::Error?) |
| 9 | db/src/metadata_overrides.rs | 15 | File 769 lines; 443 lines (net—tests removed); missing module doc; pub fns without /// on helpers | Add module doc; add /// to all pub fns; audit for >80-line functions |
| 10 | db/src/auth.rs | 14 | 14 thiserror variants (too granular); 3 password + 4 username variants collapse into Validation(String); module doc 22 lines (excess roadmap); inline test block 9 lines (trivial, ok to keep) | Consolidate to 5–6 coarse variants: InvalidCredentials, Validation(String), SessionNotFound, AccountLocked, RegistrationDisabled, Db, Hash, TokenGeneration; trim module doc |
| 11 | db/src/scanner.rs | 13 | Missing module doc entirely; pub fn list_files lacks ///; 6 inline happy-path tests, 0 error-path tests for silent fallback cases (missing path, I/O failure); duplicated make_test_dir helper | Add module doc; add one-line ///; add tests for error paths (missing_path_returns_empty, io_error_handled, etc.); move make_test_dir to test_support |
| 12 | server/src/backend.rs | 12 | File 769 lines; no module doc; handlers lack ///; mixed error handling (some anyhow, some custom); sparse coverage of 4xx/5xx paths | Add brief module doc; add /// to handler pub fns; standardize to anyhow for handlers; audit integration test coverage per 03-unit-testing.md |
| 13 | frontend/src/pages/metadata_edit.rs | 11 | File 856 lines; component with embedded logic; sparse /// on pub fns; possible refactor candidates (large match/render sections) | Add /// to pub component fn; split large render sections into named sub-components |
| 14 | server/src/auth/handlers.rs | 10 | File 600 lines; mixed error handling; sparse doc on handler pub fns | Add /// to each pub handler; standardize error wrapping |
| 15 | db/src/library_layout.rs | 9 | File 661 lines; no module doc; sparse /// coverage; 430-line net (helpers + tests); audit for function-length violations | Add module doc; add /// to pub items; audit for >80-line functions |

## Miscellaneous minor

- **db/src/auth/session.rs** (655 lines): audit module doc + doc coverage
- **db/src/settings.rs** (622 lines): missing module doc; add /// coverage
- **db/src/browse.rs** (526 lines): add module doc; audit error handling
- **db/src/indexer.rs** (508 lines): uses anyhow appropriately; audit doc coverage
- **server/src/backend/author_photos.rs** (927 lines): add module doc; audit inline tests
- **server/src/backend/overrides.rs** (594 lines): audit doc + error handling
- **frontend/src/pages/book_detail.rs** (652 lines): audit component doc + logic extraction
- **frontend/src/pages/auth.rs** (652 lines): audit component doc
- **frontend/src/pages/landing/table.rs** (627 lines): audit component doc + extraction candidates
- **frontend/src/components/search_palette.rs** (765 lines): audit component logic; split if >80-line render
- **frontend/src/components/chip_editor.rs** (484 lines): audit component doc + extraction

---

**Total files audited:** 104 Rust source files across db/, server/, frontend/, shared/, mobile/ (excluding generated code and tests.rs)

**Key patterns:** Files above 800 lines need subdirectory splits. Functions above 80 lines in high-severity files (top 5) should extract helpers. All files need module-level `//!` and pub items need `///`. Auth enum needs collapsing to coarse variants. Test blocks >150 lines move to sibling `tests.rs`.


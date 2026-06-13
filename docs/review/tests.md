## Unit & Integration Test Review

### Overall Technical Score: 68/100

> Coverage value 30 → 19: the suite is broad and behavioral, but two High gaps (a rule-mandated helper that was never written, and untested security-relevant content-validation variants on the remote-image path) and three untested error variants (Thumb `Decode`, three `FetchRemoteImageError` gates) leave real risk uncovered. Shared-helper reuse 20 → 9: the single most-repeated line (`init_db("sqlite::memory:")`, 212×), a declared-but-dormant `test-support` feature, and several copy-pasted seed/temp-dir helpers (one already drifted) are the dominant theme. Assertion correctness 20 → 14: the palette EXPLAIN-plan tests guard a private copy of the SQL, and a cluster of tautological tests inflate the count without signal. Redundancy & bloat 20 → 15: discard-then-rebuild router pattern and restatement tests. Speed 10 → 11 (capped at 10): fast overall, with one 150ms real-sleep flake. Sum: 68.

---

### High Priority — Rule-mandated in-memory pool helper missing

The project's testing rule and style guide both present `new_in_memory_pool()` as the canonical, single-source pool initializer every db test should call — the model test shape at `.claude/rules/03-unit-testing.md:65` even imports it (`use crate::test_support::{new_in_memory_pool, seed_minimal_books};`), and `docs/style-guide.md:266` names it again. The helper was never written. `grep -rn new_in_memory_pool db/src server/src frontend/src` returns only those two documentation references and zero source definitions, so the rule makes a promise the code does not keep.

In its place every db/auth test inlines the literal `init_db("sqlite::memory:").await.unwrap()` — 212 occurrences across 29 files (49 in `db/src/books/tests.rs`, 23 in `sync/tests.rs`, 20 in `palette/tests.rs`, plus auth boot/gate/extractor/handlers and 18 more). This is the single most-repeated line in the test suite.

Why it matters beyond consistency: this is future-proofing with teeth. The upcoming libraries/stats/journaling work will likely need foreign-key pragmas or `?cache=shared` on the test connection string. Today that one change requires editing 212 call sites by hand, and any that get missed will silently test against a different pragma set than production — a class of bug that passes CI and only surfaces at runtime.

Fix: add `pub async fn new_in_memory_pool() -> SqlitePool { init_db("sqlite::memory:").await.expect("in-memory pool") }` to `db/src/test_support.rs`, exported under the `test-support` feature, and migrate the call sites. The rule already says this should be one helper; the helper just needs to exist.

---

### High Priority — Remote-image content-validation variants untested

`db::author_photos::remote::FetchRemoteImageError` (`db/src/author_photos/remote.rs:26-50`) has 8 variants, but the three content-validation gates — `NotImage`, `SvgRejected`, and `TooLarge` — have zero test coverage. `db/src/author_photos/tests.rs:342-467` exercises only `BadScheme`/`BlockedAddress`/`InvalidUrl`/`BadStatus`/`Http`; a grep for the three content gates in that file returns nothing. This violates rule 03's "happy + one test per `thiserror` variant a caller can branch on."

These three variants decide whether a fetched payload is accepted as an author photo, and the same `fetch_remote_image` path is the foundation for the roadmap's cover-by-URL / uploads track — so the gap widens as that work lands. The streaming-cap branch (`remote.rs:260-267`) is security-relevant: it bounds memory against a server that omits or lies about Content-Length by aborting mid-stream the moment the running total crosses `REMOTE_IMAGE_MAX_BYTES`. There are two `TooLarge` sites — an advertised-Content-Length pre-check at `remote.rs:251` and the streaming overshoot at `remote.rs:264`. Shipping the streaming check untested means a regression that drops it (leaving only the pre-check) would pass CI silently while reintroducing an unbounded-allocation DoS.

The omission is notable because the machinery already exists. `fetch_remote_image_does_not_follow_redirects_to_private_ips` (`tests.rs:416-448`) proves a wiremock server under `RemoteImageConfig{ allow_private_addresses: true }` reaches the body-handling code.

Fix: add three wiremock-backed tests — (1) 200 with `content-type: text/html` → `NotImage` (`remote.rs:242`); (2) `content-type: image/svg+xml` → `SvgRejected` (`remote.rs:245`); (3) a body larger than `REMOTE_IMAGE_MAX_BYTES` served with no Content-Length → the streaming `TooLarge` at `remote.rs:264`.

---

### Medium Priority — db test-support feature dormant; server duplicates helpers

`db/Cargo.toml:78-83` defines a `test-support` feature whose stated purpose is to let other crates reuse `db::test_support` (in-memory seeders, `EnvVarGuard`, `indexed_audiobook`, `CoversTempDir`). No crate activates it: `server/Cargo.toml:41-50` dev-deps never list `omnibus-db = { features = ["test-support"] }`, and grep for `test-support` in the server/frontend/shared manifests returns nothing.

The consequence is that the server crate hand-rolls weaker copies of helpers that already exist and are battle-tested in db:

1. `CoversDirGuard` (`server/src/backend/test_support.rs:160-197`) duplicates `db::test_support::CoversTempDir` (`db/src/test_support.rs:145-187`) — the comment at line 152-153 admits "Mirrors the COVERS_ENV_LOCK in db::queries::tests" — and notably calls `std::env::set_var` without the `unsafe` + `// SAFETY:` documentation the db version carries.
2. `EnvGuard` (`server/src/auth/boot/tests.rs:29-93`) re-implements `db::test_support::EnvVarGuard` (`test_support.rs:58-128`) but is strictly weaker — no multi-var chaining (`also_set`), so the security-sensitive first-admin recovery tests can't set two env vars at once. A third inline `static ENV_LOCK` lives at `audiobooks/tests.rs:20`.
3. `seed_author` (`server/src/backend/test_support.rs:199-206`) and the raw-SQL audiobook seeders (`audiobooks/tests.rs:23-58`, `:251`) rebuild rows via raw `INSERT INTO books/book_files/book_file_parts` instead of reusing `db::test_support::indexed_audiobook` (`test_support.rs:287`) + `seed_synced_audiobook` (`:479`).

This is debt and a correctness liability — two diverging env-guard implementations on a security path — not a blocker, hence medium. Fix: enable `omnibus-db/test-support` in the server's dev-dependencies, then delete the server re-implementations in favor of the db helpers.

---

### Medium Priority — Core db seed and temp-dir helpers duplicated

Several cross-cutting fixtures that rule 03 says belong in `db::test_support` are copy-pasted per file instead.

1. `make_test_dir` (the pid+atomic-counter+`remove_dir_all` recipe at `db/src/test_support.rs:27-35`) is reimplemented verbatim as a private local fn in `db/src/audiobook/tests.rs:7-15` and `db/src/library_layout/tests.rs:6-14` (renamed `temp_dir`, byte-identical body) — while `db/src/indexer/tests.rs:320` correctly imports the shared one, so the helper exists and is used in one place but reinvented in two.
2. `seed_user(pool, name) -> i64` is defined three times — `progress/tests.rs:36` and `highlights/tests.rs:34` are byte-identical 6-column inserts, while `merge/tests.rs:12` is a 3-column admin variant. This has already drifted: progress/highlights set the permission columns explicitly (`is_admin, can_upload, can_edit, can_download`) while merge relies on schema defaults (`db/migrations/0004_auth.sql:22-24` are `NOT NULL DEFAULT`), so the "same" helper produces different rows — exactly the rot the rule guards against, and a future default change would silently diverge the suites.
3. `seed(pool, library, title) -> (i64, String)` (replace_books → list_books → find by title) is byte-identical between `progress/tests.rs:11` and `highlights/tests.rs:9`, and duplicates the server's `seed_book_with_uuid` (`server/src/backend/test_support.rs:91`).
4. `book_id_by_uuid` (`merge/tests.rs:21`) sits next to its sibling lookups `author_id_by_name`/`series_id_by_name`, which were already promoted to `test_support.rs:373/382` — two of three by-key lookups are shared and the third isn't.

Fix: hoist `seed_user(pool, name)`, `seed_book_with_uuid(pool, library, title)`, and `book_id_by_uuid` into `db/src/test_support.rs` (pick one canonical user-row shape), delete the per-file copies, and point library_layout's `temp_dir` at the shared `make_test_dir`. These will spread further as Phase-4 journaling/ratings tests need user+book fixtures.

---

### Medium Priority — Palette EXPLAIN tests assert copy-pasted SQL

`palette_taxonomy_query_plans_use_indexes` (`db/src/palette/tests.rs:887-1021`) re-types the three production taxonomy queries (authors/series/tags) as ~90 lines of inline backslash-continued SQL literals (`tests.rs:909-1015`), then runs EXPLAIN QUERY PLAN on those literals to assert no full SCAN of the link tables (asserts at `:940/:978/:1017`). The production queries live independently in `db/src/palette/authors.rs:46-80`, `series.rs:44`, and `tags.rs:40` as their own `r#"..."#` literals — there is no shared constant binding them. A grep for `WITH effective` returns the three production files plus the test, confirming no shared source.

So the test validates the query plan of its own private copy of the SQL, not the statement the application actually runs. If someone edits the real query in `authors.rs` — drops a JOIN, reorders the WHERE, mutates the CTE so it full-scans the link table — the test keeps passing against the stale duplicate, and the index-regression guard it exists to provide silently vanishes. This is the most expensive false-confidence test in the suite: it looks like a strong structural guard but is decoupled from the code it guards.

Fix: hoist each query into a `pub(crate) const` (or `fn ..._sql() -> &'static str`) in `authors.rs`/`series.rs`/`tags.rs`, and have both the production read path and the EXPLAIN test reference the same constant, so the planner check runs against the real statement. A miniature version of the same drift exists in the auth session prune-plan tests (`session.rs`), but those re-type only a single short DELETE, so the exposure is far smaller.

---

### Medium Priority — Thumbnail decode-failure variant has no test

`db::thumbs::ThumbError` (`db/src/thumbs.rs:57-69`) has five variants — `Decode`, `Encode`, `Io`, `NoCover`, `Db` — but `db/src/thumbs/tests.rs` only exercises the happy path of `generate_thumbnail` (`tests.rs:73-95`, asserting valid WebP output). A grep for `ThumbError` or any error-variant assertion in that file returns nothing. Per rule 03 every pub fn gets one test per `thiserror` variant it can return, and `generate_thumbnail` can return `Decode` (on `image::load_from_memory` failure at `thumbs.rs:176`) and `Encode`.

The `Decode` gap matters because it is the single most likely real-world failure: a corrupt or non-image cover flowing into the on-demand thumbnail pipeline. Thumbnails are generated on demand for the library grid — a regression that panics instead of returning `Decode` on bad input would surface as a 500 on the hot browse path rather than a graceful skip.

Fix: add a one-line test passing `b"not an image"` (or a truncated PNG header) to `generate_thumbnail` and assert `matches!(err, ThumbError::Decode(_))`. `NoCover(i64)` (`thumbs.rs:66`) on the book-level generate path is also untested but lower-value; `Encode`/`Io` are hard to trigger deterministically and can reasonably stay uncovered.

---

### Medium Priority — Thumbnail eviction test sleeps and trusts mtime

`evict_if_over_cap_removes_oldest_files` (`db/src/thumbs/tests.rs:97-115`) tests FIFO-by-mtime eviction by writing three files with `std::thread::sleep(Duration::from_millis(50))` between each (150ms+ wall-clock per run, `tests.rs:104-114`) and trusting the OS to record strictly increasing modification times. It is flaky on two axes: (1) filesystems with coarse mtime granularity — some network/older FS, CI containers — may stamp two writes with the same resolution, making which file is evicted nondeterministic; (2) the fixed sleep is the suite's only unbounded real-time wait outside the worker module (which uses bounded poll loops), and grep confirms this is the only `thread::sleep` in any db/server/frontend test file.

The test author flagged the fragility in the comment ("we can only guarantee ordering, not specific times"). It is also under-asserting: it only checks `remaining.len() == 2`, so it confirms the cap arithmetic but never pins *which* file survived — the actual FIFO contract.

Fix: set deterministic mtimes via `File::set_modified` — the pattern `backdate` already uses at `db/src/hls/tests.rs:20-30` — instead of sleeping, and assert the surviving file by name. This removes both the wall-clock cost and the ordering nondeterminism in one change.

---

### Low Priority — Author-insert seed duplicated across db and server

The `INSERT INTO authors (name, sort) ... RETURNING id` seed is reimplemented in test code at `db/src/discovery/tests.rs:255`, `db/src/author_photos/tests.rs:24` (inside `pool_with_author`) plus three more inline at `:496/:502/:508`, inside `db/src/test_support.rs:405` (the larger `seed_books_for_one_author_and_series`), and `server/src/backend/test_support.rs:200`. `db::test_support` already hosts a lookup-only `author_id_by_name` (`test_support.rs:373`) but no `seed_author` insert counterpart, so each consumer rolls its own.

The `pool_with_author(name) -> (SqlitePool, i64)` wrapper at `author_photos/tests.rs:22-31` is literally `init_db("sqlite::memory:")` + that author insert — i.e. a two-line composition of the two helpers this review recommends creating (`new_in_memory_pool` + `seed_author`).

Fix: add `pub async fn seed_author(pool, name) -> i64` to `db/src/test_support.rs` and route discovery, author_photos, and (via the test-support feature) the server through it. Low priority because each instance is small and correct, but author seeding will be needed broadly once the author-pages / personalization phases land.

---

### Low Priority — Backend tests discard fixture app and rebuild router

`test_support::fixture()` (`server/src/backend/test_support.rs:10-17`) returns `(Router, AppState, SqlitePool)` with a cloned pool, so a test can seed via the returned pool and drive the already-built app. Many backend tests defeat this: they call `let (_, _, pool) = fixture().await;` — discarding the just-built app — then seed and rebuild an identical router via `crate::backend::rest_router(AppState::new(pool))`.

Because the fixture's app holds a clone of the same pool, the rebuild is unnecessary: rows seeded after `fixture()` are already visible to the original app. `audiobooks/tests.rs:87` discards the app, `:90` seeds, `:92` rebuilds — the seed would be visible without rebuilding. The pattern appears 16 times in `audiobooks/tests.rs` (16 `rest_router(AppState::new(pool))` rebuilds matching 16 `let (_, _, pool) = fixture()` discards) and 5 times in `ebooks/tests.rs`, and is applied inconsistently — neighboring tests in the same file use the fixture app directly — so a reader can't tell whether a given rebuild is meaningful.

Lower correctness risk than the other findings, but it's the most-repeated structural inconsistency in the server tests and doubles router construction per affected test. Fix: standardize on `fixture()` (seed the returned pool, use the returned app), or add a `fixture_seeded(seed_fn)` helper, and remove the inline rebuilds.

---

### Low Priority — Cluster of restatement and tautological tests

A set of cheap tests assert things the type system, a derive macro, or a one-line `match` already guarantees, so they can only fail when someone edits the literal they mirror — at which point they're updated mechanically, never on a real behavior bug. They pad the variant/coverage count without reducing risk.

1. `audiobook_error_io_variant_renders_useful_message` / `audiobook_error_unsupported_variant_renders_format_token` (`db/src/audiobook/tests.rs:115-128`) assert the `#[error("...")]` Display strings of `AudiobookError`; `Unsupported` is never constructed in production (only defined at `parse.rs:32`), so they aren't even smoke coverage of a branch — the real contract (a bad file yields an error row, not a panic) is already pinned by `parse_unreadable_file_surfaces_as_error_metadata_row` (`tests.rs:66`).
2. `palette_duration_populated` (`db/src/palette/tests.rs:472-486`) builds a full fixture only to assert `duration_ms < 10000` (`:485`) — a 10s ceiling no single-book in-memory path could exceed, and which a `duration_ms` of 0 always passes, so it never fires.
3. Frontend enum→&str map tests that re-type their own match table: banner `class_suffix_covers_every_kind`/`icon_glyphs_match_kind` (`banner.rs:103-125` vs match at `:21-46`), strength `tier_class_covers_each_score` (`strength.rs:96-109`), and the weakest, worker_status `kind_label_covers_all_variants_in_both_tenses` (`worker_status.rs:243-255`) which asserts only `!is_empty()` and would pass even if two variants mapped to the same wrong label.
4. StrengthScore `passes_through_in_range` (`strength.rs:89-94`) and `from_u8_matches_new` (`:116-120`) — clamping is already covered by `clamps_above_max` and `tier_class_saturates_on_overflow_input`.
5. The `ssr_tests` no-op modules in `view_prefs.rs:167-206` / `reader_progress.rs:152-162` / `audiobook_progress.rs:220-235` assert that an empty `save_impl(){}` is inert and a constant `load_impl` returns the constant.

Stated honestly: each is cheap and harmless, so this is polish, not debt. Keep the genuinely behavioral siblings — banner `error_uses_alert_role_others_use_status` pins a real a11y rule, the icon test pins the non-obvious Err+Warn→"!" collapse, the mobile-store round-trip tests exercise real code, and view_prefs `default_prefs_match_documented_shape` pins the SSR first-hydration markup shape. Fix direction: delete the pure restatements (the audiobook Display tests, `palette_duration`'s assertion, the StrengthScore trivial pair, the `save_is_a_noop` SSR tests); for worker_status `kind_label` either drop it or strengthen it to assert the actual strings.

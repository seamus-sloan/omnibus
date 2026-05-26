//! Inline integration tests for the db query layer. Lives here as a
//! single file (rather than co-located per module) because so many tests
//! exercise behavior that spans two or three of the new modules
//! (covers + sync, settings + cover cleanup, overrides + book reads).
//! Splitting them per module would force every test helper out into
//! `pub(crate)` and obscure what's actually under test. Future tests for
//! a single module's internal behavior should still land in that
//! module's inline `#[cfg(test)] mod tests`.

mod tests {
    use crate::author_photos_data::{
        author_photo_status, delete_author, delete_author_photo, get_author_photo,
        upsert_author_photo, AuthorPhotoSource,
    };
    use crate::books::{
        count_books, get_book, library_from_db, library_from_db_with_total, list_books,
        list_indexed_rows, sanitize_description, search_books, MAX_BOOKS_RETURNED,
    };
    use crate::browse::{list_authors, list_series};
    use crate::covers::test_helpers::CoversTempDir;
    use crate::covers::{cover_path_for, delete_cover_files_for, get_cover, write_cover_file};
    use crate::discovery::{get_author, get_series, get_tag_cloud, MAX_DISCOVERY_BOOKS};
    use crate::ebook::IndexedBook;
    use crate::helpers::stable_uuid;
    use crate::metadata_overrides::{
        delete_metadata_overrides, get_metadata_overrides, merge_metadata_overrides,
        upsert_metadata_overrides, write_override_cover,
    };
    use crate::palette::search_palette;
    use crate::pool::init_db;
    use crate::settings::{last_indexed_at, prune_orphan_libraries, set_settings, Settings};
    use crate::sync::{replace_books, sync_books, SyncPlan};
    use omnibus_shared::{Contributor, EbookMetadata, Identifier, MetadataOverrides};
    use sqlx::{Row, SqlitePool};

    fn indexed(
        filename: &str,
        title: Option<&str>,
        authors: &[&str],
        subjects: &[&str],
        series: Option<(&str, &str)>,
        cover: Option<(&str, &[u8])>,
    ) -> IndexedBook {
        IndexedBook {
            metadata: EbookMetadata {
                filename: filename.into(),
                title: title.map(Into::into),
                creators: authors
                    .iter()
                    .map(|a| Contributor {
                        name: (*a).into(),
                        ..Default::default()
                    })
                    .collect(),
                subjects: subjects.iter().map(|s| (*s).to_string()).collect(),
                series: series.map(|(n, _)| n.into()),
                series_index: series.map(|(_, i)| i.into()),
                ..Default::default()
            },
            cover: cover.map(|(m, b)| (m.into(), b.to_vec())),
            mtime_epoch: 0,
            size_bytes: 0,
        }
    }

    #[tokio::test]
    async fn replace_books_inserts_metadata_and_covers() {
        let _covers = CoversTempDir::new("insert");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed(
                    "a.epub",
                    Some("A"),
                    &["Author A"],
                    &["fiction"],
                    Some(("Saga", "1")),
                    Some(("image/jpeg", b"BYTES")),
                ),
                indexed("b.epub", Some("B"), &["Author B"], &[], None, None),
            ],
        )
        .await
        .expect("replace should succeed");

        let books = list_books(&pool, "/lib").await.unwrap();
        assert_eq!(books.len(), 2);

        let a = books
            .iter()
            .find(|b| b.title.as_deref() == Some("A"))
            .unwrap();
        let b = books
            .iter()
            .find(|b| b.title.as_deref() == Some("B"))
            .unwrap();

        assert_eq!(a.filename, "a.epub");
        assert_eq!(b.filename, "b.epub");
        assert_eq!(a.creators.len(), 1);
        assert_eq!(a.creators[0].name, "Author A");
        assert_eq!(a.subjects, vec!["fiction".to_string()]);
        assert_eq!(a.series.as_deref(), Some("Saga"));
        assert_eq!(a.series_index.as_deref(), Some("1"));

        let a_uuid = a.unique_identifier.clone().unwrap();
        assert_eq!(
            a.cover_url.as_deref(),
            Some(format!("/api/covers/{a_uuid}").as_str())
        );
        assert_eq!(b.cover_url, None);

        // F1.3: list_books exposes the row insertion timestamp so the
        // landing page can offer a "Newest Added" sort. The migration
        // defaults `books.timestamp` to `datetime('now')`
        // (`YYYY-MM-DD HH:MM:SS`, UTC), so every row surfaces a non-empty
        // sortable string.
        for book in &books {
            let added = book.added_at.as_deref().unwrap_or("");
            assert!(
                !added.is_empty(),
                "added_at should be populated for {:?}",
                book.title
            );
        }

        let cover = get_cover(&pool, a.id).await.unwrap();
        assert_eq!(cover, Some(("image/jpeg".into(), b"BYTES".to_vec())));
        assert!(get_cover(&pool, b.id).await.unwrap().is_none());

        assert!(last_indexed_at(&pool, "/lib").await.unwrap().is_some());
    }

    /// F1.7 Atrium accent round-trip. `replace_books` writes
    /// `metadata.accent` into `books.accent_color`; `list_books` /
    /// `get_book` / `search_books` read it back into
    /// `EbookMetadata.accent`. Verify the column survives the trip and
    /// `None` stays `None` (not coerced to an empty string).
    #[tokio::test]
    async fn replace_books_round_trips_accent_color() {
        let _covers = CoversTempDir::new("accent_round_trip");
        let pool = init_db("sqlite::memory:").await.unwrap();

        let with_accent = IndexedBook {
            metadata: EbookMetadata {
                filename: "with-accent.epub".into(),
                title: Some("Piranesi".into()),
                creators: vec![Contributor {
                    name: "Susanna Clarke".into(),
                    ..Default::default()
                }],
                accent: Some("oklch(0.660 0.130 245.0)".into()),
                ..Default::default()
            },
            cover: None,
            mtime_epoch: 0,
            size_bytes: 0,
        };
        let no_accent = IndexedBook {
            metadata: EbookMetadata {
                filename: "no-accent.epub".into(),
                title: Some("Plain".into()),
                creators: vec![Contributor {
                    name: "Anon".into(),
                    ..Default::default()
                }],
                accent: None,
                ..Default::default()
            },
            cover: None,
            mtime_epoch: 0,
            size_bytes: 0,
        };
        replace_books(&pool, "/lib", vec![with_accent, no_accent])
            .await
            .expect("replace should succeed");

        // list_books returns the accent column for every row.
        let books = list_books(&pool, "/lib").await.unwrap();
        let p = books
            .iter()
            .find(|b| b.title.as_deref() == Some("Piranesi"))
            .unwrap();
        let plain = books
            .iter()
            .find(|b| b.title.as_deref() == Some("Plain"))
            .unwrap();
        assert_eq!(p.accent.as_deref(), Some("oklch(0.660 0.130 245.0)"));
        assert_eq!(plain.accent, None);

        // get_book returns the same value through the single-row path.
        let detail = get_book(&pool, p.id).await.unwrap().unwrap();
        assert_eq!(detail.accent.as_deref(), Some("oklch(0.660 0.130 245.0)"));
        let detail_plain = get_book(&pool, plain.id).await.unwrap().unwrap();
        assert_eq!(detail_plain.accent, None);
    }

    /// #125: the write-boundary gate must accept the exact `oklch(L C H)`
    /// shape the indexer emits, and reject anything else — including raw
    /// hex, CSS keywords, and injection payloads that try to break out of
    /// the `style="background: {bg}"` attribute used by Atrium consumers.
    ///
    /// End-to-end gate: writing an `IndexedBook` whose `accent` carries an
    /// injection payload must result in `accent_color = NULL` in the DB,
    /// not the unsanitized string.
    #[tokio::test]
    async fn replace_books_drops_unsafe_accent_color() {
        let _covers = CoversTempDir::new("accent_unsafe");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let unsafe_book = IndexedBook {
            metadata: EbookMetadata {
                filename: "shady.epub".into(),
                title: Some("Shady".into()),
                creators: vec![Contributor {
                    name: "Anon".into(),
                    ..Default::default()
                }],
                accent: Some("red; background: url(x)".into()),
                ..Default::default()
            },
            cover: None,
            mtime_epoch: 0,
            size_bytes: 0,
        };
        replace_books(&pool, "/lib", vec![unsafe_book])
            .await
            .expect("replace should succeed");
        let books = list_books(&pool, "/lib").await.unwrap();
        let shady = books
            .iter()
            .find(|b| b.title.as_deref() == Some("Shady"))
            .unwrap();
        assert_eq!(shady.accent, None);
    }

    // ------------------------------------------------------------------
    // sync_books — incremental write path. Each test seeds an initial
    // state via `replace_books` (the legacy nuke-and-pave wrapper), then
    // applies a hand-built `SyncPlan` to exercise one or more diff
    // buckets and asserts the post-state.
    // ------------------------------------------------------------------

    /// Build an `IndexedBook` matching `indexed(...)` but with the
    /// supplied (mtime_epoch, size_bytes). Used to drive the New +
    /// Changed branches of sync_books with realistic fs metadata.
    fn indexed_with_stat(
        filename: &str,
        title: Option<&str>,
        mtime_epoch: i64,
        size_bytes: i64,
    ) -> IndexedBook {
        IndexedBook {
            metadata: EbookMetadata {
                filename: filename.into(),
                title: title.map(Into::into),
                ..Default::default()
            },
            cover: None,
            mtime_epoch,
            size_bytes,
        }
    }

    /// Seed two books via `replace_books`, then `sync_books` with no
    /// diff buckets at all. Both ids must survive — that's the whole
    /// point of the refactor.
    #[tokio::test]
    async fn sync_preserves_book_id_for_unchanged() {
        let _covers = CoversTempDir::new("sync_unchanged");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed("a.epub", Some("A"), &[], &[], None, None),
                indexed("b.epub", Some("B"), &[], &[], None, None),
            ],
        )
        .await
        .unwrap();
        let before: Vec<_> = list_books(&pool, "/lib")
            .await
            .unwrap()
            .into_iter()
            .map(|b| (b.filename.clone(), b.id))
            .collect();

        sync_books(&pool, "/lib", SyncPlan::default())
            .await
            .unwrap();

        let after: Vec<_> = list_books(&pool, "/lib")
            .await
            .unwrap()
            .into_iter()
            .map(|b| (b.filename.clone(), b.id))
            .collect();
        assert_eq!(before, after, "ids must be preserved across a no-op sync");
    }

    #[tokio::test]
    async fn sync_preserves_book_id_for_changed() {
        let _covers = CoversTempDir::new("sync_changed");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "a.epub",
                Some("Old Title"),
                &["Old Author"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();
        let original_id = list_books(&pool, "/lib").await.unwrap()[0].id;

        // One Changed entry — same filename so same uuid, new title + author.
        let plan = SyncPlan {
            changed_books: vec![IndexedBook {
                metadata: EbookMetadata {
                    filename: "a.epub".into(),
                    title: Some("New Title".into()),
                    creators: vec![Contributor {
                        name: "New Author".into(),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                cover: None,
                mtime_epoch: 999,
                size_bytes: 42,
            }],
            ..Default::default()
        };
        sync_books(&pool, "/lib", plan).await.unwrap();

        let after = list_books(&pool, "/lib").await.unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].id, original_id, "books.id must be preserved");
        assert_eq!(after[0].title.as_deref(), Some("New Title"));
        assert_eq!(after[0].creators.len(), 1);
        assert_eq!(after[0].creators[0].name, "New Author");
    }

    /// A user-supplied metadata override (keyed by `book_uuid`, no FK to
    /// `books.id`) must still apply after a Changed UPDATE — proving
    /// that the in-place UPDATE doesn't accidentally rotate the uuid
    /// and that the overrides table isn't touched by sync_books.
    #[tokio::test]
    async fn sync_overrides_survive_changed() {
        let _covers = CoversTempDir::new("sync_overrides");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;
        replace_books(
            &pool,
            "/lib",
            vec![indexed("a.epub", Some("Scanned"), &[], &[], None, None)],
        )
        .await
        .unwrap();
        let book_uuid = list_indexed_rows(&pool, "/lib").await.unwrap()[0]
            .uuid
            .clone();

        // Write a user override that renames the title.
        let overrides = MetadataOverrides {
            title: Some("User Title".into()),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &book_uuid, &overrides, false, user_id)
            .await
            .unwrap();

        // Now Change the book — the scan would happily say "Scanned"
        // again, but the override should still surface "User Title".
        let plan = SyncPlan {
            changed_books: vec![indexed_with_stat("a.epub", Some("Scanned v2"), 100, 100)],
            ..Default::default()
        };
        sync_books(&pool, "/lib", plan).await.unwrap();

        let after = list_books(&pool, "/lib").await.unwrap();
        assert_eq!(after[0].title.as_deref(), Some("User Title"));
    }

    /// A Removed uuid must wipe books_fts, book_files,
    /// books_authors_link, etc. — the cascade plus our explicit FTS
    /// clear should leave no orphans.
    #[tokio::test]
    async fn sync_removes_book_cascades_links_and_fts() {
        let _covers = CoversTempDir::new("sync_removed_cascade");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "doomed.epub",
                Some("Doomed"),
                &["Anon"],
                &["fic"],
                None,
                None,
            )],
        )
        .await
        .unwrap();
        let book_id = list_books(&pool, "/lib").await.unwrap()[0].id;
        let uuid = list_indexed_rows(&pool, "/lib").await.unwrap()[0]
            .uuid
            .clone();

        let plan = SyncPlan {
            removed_uuids: vec![uuid],
            ..Default::default()
        };
        sync_books(&pool, "/lib", plan).await.unwrap();

        let books_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books WHERE id = ?")
            .bind(book_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        let files_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM book_files WHERE book_id = ?")
                .bind(book_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        let link_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM books_authors_link WHERE book = ?")
                .bind(book_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        let fts_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books_fts WHERE rowid = ?")
            .bind(book_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(books_count, 0);
        assert_eq!(files_count, 0);
        assert_eq!(link_count, 0);
        assert_eq!(fts_count, 0);
    }

    /// One sync covering all four mutating branches at once. Unchanged
    /// ids stay put; Changed id stays put; New gets a fresh id;
    /// Removed disappears.
    #[tokio::test]
    async fn sync_mixed_diff_in_one_transaction() {
        let _covers = CoversTempDir::new("sync_mixed");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed("keep.epub", Some("Keep"), &[], &[], None, None),
                indexed("edit.epub", Some("Old Edit"), &[], &[], None, None),
                indexed("gone.epub", Some("Gone"), &[], &[], None, None),
            ],
        )
        .await
        .unwrap();
        let before: std::collections::HashMap<String, i64> = list_books(&pool, "/lib")
            .await
            .unwrap()
            .into_iter()
            .map(|b| (b.filename.clone(), b.id))
            .collect();
        let gone_uuid = stable_uuid("/lib", "gone.epub");

        let plan = SyncPlan {
            new_books: vec![indexed_with_stat("add.epub", Some("Added"), 100, 100)],
            changed_books: vec![indexed_with_stat("edit.epub", Some("New Edit"), 200, 200)],
            removed_uuids: vec![gone_uuid],
            backfill: vec![],
        };
        sync_books(&pool, "/lib", plan).await.unwrap();

        let after: std::collections::HashMap<String, i64> = list_books(&pool, "/lib")
            .await
            .unwrap()
            .into_iter()
            .map(|b| (b.filename.clone(), b.id))
            .collect();

        assert_eq!(after.len(), 3);
        assert_eq!(after.get("keep.epub"), before.get("keep.epub"));
        assert_eq!(after.get("edit.epub"), before.get("edit.epub"));
        assert!(after.contains_key("add.epub"));
        assert!(!after.contains_key("gone.epub"));
    }

    /// Removed books should lose their cover files; survivors' covers
    /// must stay intact. Catches "delete every cover on every sync"
    /// regressions if anyone ever short-circuits the bucket logic.
    #[tokio::test]
    async fn sync_cover_sidecar_lifecycle_on_remove() {
        let covers = CoversTempDir::new("sync_cover_remove");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed(
                    "keep.epub",
                    Some("Keep"),
                    &[],
                    &[],
                    None,
                    Some(("image/jpeg", b"KEEP_BYTES")),
                ),
                indexed(
                    "gone.epub",
                    Some("Gone"),
                    &[],
                    &[],
                    None,
                    Some(("image/jpeg", b"GONE_BYTES")),
                ),
            ],
        )
        .await
        .unwrap();
        let keep_uuid = stable_uuid("/lib", "keep.epub");
        let gone_uuid = stable_uuid("/lib", "gone.epub");
        let keep_path = covers.path.join(format!("{keep_uuid}.jpg"));
        let gone_path = covers.path.join(format!("{gone_uuid}.jpg"));
        assert!(keep_path.exists(), "cover for keep should exist");
        assert!(gone_path.exists(), "cover for gone should exist");

        sync_books(
            &pool,
            "/lib",
            SyncPlan {
                removed_uuids: vec![gone_uuid],
                ..Default::default()
            },
        )
        .await
        .unwrap();

        assert!(keep_path.exists(), "survivor cover must remain");
        assert!(!gone_path.exists(), "removed cover must be deleted");
    }

    /// FTS5 row carries `rowid = books.id`. After a Changed UPDATE the
    /// rowid must still equal the preserved id, and the index content
    /// must reflect the new title.
    #[tokio::test]
    async fn sync_fts_row_consistent_after_changed() {
        let _covers = CoversTempDir::new("sync_fts");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed("a.epub", Some("Antarctica"), &[], &[], None, None)],
        )
        .await
        .unwrap();
        let original_id = list_books(&pool, "/lib").await.unwrap()[0].id;

        sync_books(
            &pool,
            "/lib",
            SyncPlan {
                changed_books: vec![indexed_with_stat("a.epub", Some("Borealis"), 200, 200)],
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let stale = search_books(&pool, "/lib", "Antarctica").await.unwrap();
        let fresh = search_books(&pool, "/lib", "Borealis").await.unwrap();
        assert!(stale.is_empty(), "old title must not match after change");
        assert_eq!(fresh.len(), 1);
        assert_eq!(fresh[0].id, original_id, "FTS rowid stable across change");
    }

    /// The Backfill bucket fills in the post-migration sentinel stat
    /// values without touching any metadata columns. Confirm both
    /// invariants: stat populated, OPF-derived fields untouched.
    #[tokio::test]
    async fn sync_backfill_writes_stat_without_touching_metadata() {
        let _covers = CoversTempDir::new("sync_backfill");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed("a.epub", Some("Original"), &[], &[], None, None)],
        )
        .await
        .unwrap();
        let uuid = list_indexed_rows(&pool, "/lib").await.unwrap()[0]
            .uuid
            .clone();
        // Confirm the row started at the (0, 0) sentinel — replace_books
        // wrote the IndexedBook stats (which the test fixture defaults
        // to 0).
        let pre = list_indexed_rows(&pool, "/lib").await.unwrap();
        assert_eq!(pre[0].mtime_epoch, 0);
        assert_eq!(pre[0].size_bytes, 0);

        sync_books(
            &pool,
            "/lib",
            SyncPlan {
                backfill: vec![(uuid, 1234, 5678)],
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let post = list_indexed_rows(&pool, "/lib").await.unwrap();
        assert_eq!(post[0].mtime_epoch, 1234);
        assert_eq!(post[0].size_bytes, 5678);
        // Title is untouched — backfill must not have triggered any
        // metadata writes.
        let books = list_books(&pool, "/lib").await.unwrap();
        assert_eq!(books[0].title.as_deref(), Some("Original"));
    }

    /// Empty disk → diff says "remove all" → sync_books wipes the
    /// library cleanly. Stress test for the Removed branch.
    #[tokio::test]
    async fn sync_empty_plan_with_full_removed_clears_library() {
        let _covers = CoversTempDir::new("sync_empty");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed("a.epub", Some("A"), &[], &[], None, None),
                indexed("b.epub", Some("B"), &[], &[], None, None),
            ],
        )
        .await
        .unwrap();
        let all_uuids: Vec<String> = list_indexed_rows(&pool, "/lib")
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.uuid)
            .collect();

        sync_books(
            &pool,
            "/lib",
            SyncPlan {
                removed_uuids: all_uuids,
                ..Default::default()
            },
        )
        .await
        .unwrap();

        assert!(list_books(&pool, "/lib").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn reindex_replaces_library_atomically() {
        let _covers = CoversTempDir::new("atomic");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "a.epub",
                Some("A"),
                &["Author A"],
                &["fiction"],
                None,
                Some(("image/jpeg", b"OLD")),
            )],
        )
        .await
        .unwrap();

        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "a.epub",
                Some("A"),
                &["Author A"],
                &["fiction"],
                None,
                Some(("image/jpeg", b"NEW")),
            )],
        )
        .await
        .unwrap();

        // No orphan rows in any link table for books that no longer exist.
        for table in [
            "books_authors_link",
            "books_tags_link",
            "books_series_link",
            "books_publishers_link",
            "books_languages_link",
        ] {
            let orphan: i64 = sqlx::query_scalar(&format!(
                "SELECT COUNT(*) FROM {table} WHERE book NOT IN (SELECT id FROM books)"
            ))
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(orphan, 0, "{table} should have no orphans");
        }
        let orphan_files: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM book_files WHERE book_id NOT IN (SELECT id FROM books)",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(orphan_files, 0);

        let books = list_books(&pool, "/lib").await.unwrap();
        assert_eq!(books.len(), 1);
        let cover = get_cover(&pool, books[0].id).await.unwrap();
        assert_eq!(cover, Some(("image/jpeg".into(), b"NEW".to_vec())));
    }

    #[tokio::test]
    async fn author_dedupes_across_books_case_insensitive() {
        let _covers = CoversTempDir::new("dedupe");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed("a.epub", Some("A"), &["Tolkien"], &[], None, None),
                indexed("b.epub", Some("B"), &["tolkien"], &[], None, None),
            ],
        )
        .await
        .unwrap();
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM authors")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 1, "NOCASE unique should collapse Tolkien/tolkien");
    }

    #[tokio::test]
    async fn series_index_sorts_numerically() {
        // Regression guard against reintroducing Calibre's TEXT series_index:
        // 10 must sort after 2, not before.
        let _covers = CoversTempDir::new("series");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed("b.epub", Some("B"), &["A"], &[], Some(("S", "10")), None),
                indexed("a.epub", Some("A"), &["A"], &[], Some(("S", "2")), None),
            ],
        )
        .await
        .unwrap();
        let indices: Vec<f64> =
            sqlx::query_scalar("SELECT series_index FROM books ORDER BY series_index")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(indices, vec![2.0, 10.0]);
    }

    #[tokio::test]
    async fn cover_returns_none_when_file_missing() {
        let _covers = CoversTempDir::new("missing");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "a.epub",
                Some("A"),
                &["A"],
                &[],
                None,
                Some(("image/jpeg", b"BYTES")),
            )],
        )
        .await
        .unwrap();
        let books = list_books(&pool, "/lib").await.unwrap();
        let uuid: String = sqlx::query_scalar("SELECT uuid FROM books WHERE id = ?")
            .bind(books[0].id)
            .fetch_one(&pool)
            .await
            .unwrap();
        // Remove the file out from under the DB — get_cover should report
        // None, not error.
        let _ = std::fs::remove_file(cover_path_for(&uuid, "jpg"));
        assert!(get_cover(&pool, books[0].id).await.unwrap().is_none());
    }

    /// F5.1 / #107: when a `metadata_overrides` row sets
    /// `has_cover_override = 1` and an `override-<uuid>.<ext>` file exists
    /// on disk, `get_cover` returns the override bytes — not the scanned
    /// cover. Single-query form must preserve this precedence.
    #[tokio::test]
    async fn cover_returns_override_when_flag_set() {
        let _covers = CoversTempDir::new("override_set");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "a.epub",
                Some("A"),
                &["A"],
                &[],
                None,
                Some(("image/jpeg", b"ORIGINAL")),
            )],
        )
        .await
        .unwrap();

        let books = list_books(&pool, "/lib").await.unwrap();
        let uuid = books[0].unique_identifier.clone().unwrap();

        // Mark cover-override + drop the override file on disk.
        write_override_cover(&uuid, "image/png", b"OVERRIDE").unwrap();
        upsert_metadata_overrides(&pool, &uuid, &MetadataOverrides::default(), true, user_id)
            .await
            .unwrap();

        let cover = get_cover(&pool, books[0].id).await.unwrap();
        assert_eq!(cover, Some(("image/png".into(), b"OVERRIDE".to_vec())));
    }

    /// F5.1 / #107: with no `metadata_overrides` row, `get_cover` falls
    /// through to the scanned `<uuid>.<ext>` cover. The LEFT JOIN must
    /// not filter the book out when no override row exists.
    #[tokio::test]
    async fn cover_returns_original_when_no_override_row() {
        let _covers = CoversTempDir::new("override_absent");
        let pool = init_db("sqlite::memory:").await.unwrap();

        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "a.epub",
                Some("A"),
                &["A"],
                &[],
                None,
                Some(("image/jpeg", b"ORIGINAL")),
            )],
        )
        .await
        .unwrap();

        let books = list_books(&pool, "/lib").await.unwrap();
        let cover = get_cover(&pool, books[0].id).await.unwrap();
        assert_eq!(cover, Some(("image/jpeg".into(), b"ORIGINAL".to_vec())));
    }

    /// F5.1 / #107: a `metadata_overrides` row with
    /// `has_cover_override = 0` (text-only edits, no cover swap) must
    /// resolve to the scanned cover, not the override path.
    #[tokio::test]
    async fn cover_returns_original_when_override_flag_unset() {
        let _covers = CoversTempDir::new("override_flag_off");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "a.epub",
                Some("A"),
                &["A"],
                &[],
                None,
                Some(("image/jpeg", b"ORIGINAL")),
            )],
        )
        .await
        .unwrap();

        let books = list_books(&pool, "/lib").await.unwrap();
        let uuid = books[0].unique_identifier.clone().unwrap();

        // Override row exists with text edits but no cover swap.
        upsert_metadata_overrides(
            &pool,
            &uuid,
            &MetadataOverrides {
                title: Some("Edited".into()),
                ..Default::default()
            },
            false,
            user_id,
        )
        .await
        .unwrap();

        let cover = get_cover(&pool, books[0].id).await.unwrap();
        assert_eq!(cover, Some(("image/jpeg".into(), b"ORIGINAL".to_vec())));
    }

    /// #107: `get_cover` for a non-existent book id returns `Ok(None)`
    /// (not an error). The LEFT JOIN must not change this contract.
    #[tokio::test]
    async fn cover_returns_none_for_missing_book_id() {
        let _covers = CoversTempDir::new("missing_book");
        let pool = init_db("sqlite::memory:").await.unwrap();
        assert!(get_cover(&pool, 999_999).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn library_from_db_returns_empty_for_none_path() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let lib = library_from_db(&pool, None).await.unwrap();
        assert!(lib.path.is_none());
        assert!(lib.books.is_empty());
        assert!(lib.error.is_none());
    }

    // ---------- Server-side cap (issue #81) ----------
    //
    // `list_books` / `search_books` previously had no `LIMIT`, so a single
    // `/api/ebooks` poll on a multi-thousand-book library serialized the
    // whole table. The fix is a hard `LIMIT MAX_BOOKS_RETURNED`, plus a
    // companion count helper so callers can detect truncation.

    /// Seed `count` minimal `books` rows under `/lib` using a recursive CTE.
    /// Bypasses `replace_books` / the indexer entirely — the cap behavior
    /// only depends on rows existing, not on full m2m relations being set
    /// up. Keeps the test runtime down to milliseconds even for 50k+ rows.
    async fn seed_minimal_books(pool: &SqlitePool, count: i64) {
        sqlx::query("INSERT INTO libraries (path, display_name) VALUES ('/lib', 'lib')")
            .execute(pool)
            .await
            .unwrap();
        let lib_id: i64 = sqlx::query_scalar("SELECT id FROM libraries WHERE path = '/lib'")
            .fetch_one(pool)
            .await
            .unwrap();
        sqlx::query(
            r#"
            WITH RECURSIVE n(i) AS (
                SELECT 1
                UNION ALL
                SELECT i + 1 FROM n WHERE i < ?
            )
            INSERT INTO books (uuid, library_id, path, title, sort)
            SELECT 'uuid-' || i, ?, '/lib/b' || i, 'Title ' || i,
                   'Title ' || printf('%010d', i)
              FROM n
            "#,
        )
        .bind(count)
        .bind(lib_id)
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn list_books_caps_response_at_max_books_returned() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let total = MAX_BOOKS_RETURNED + 25;
        seed_minimal_books(&pool, total).await;

        let books = list_books(&pool, "/lib").await.unwrap();
        assert_eq!(
            books.len() as i64,
            MAX_BOOKS_RETURNED,
            "list_books must cap the returned vec at MAX_BOOKS_RETURNED"
        );

        let counted = count_books(&pool, "/lib").await.unwrap();
        assert_eq!(
            counted, total,
            "count_books must report the true row count (uncapped)"
        );
    }

    #[tokio::test]
    async fn count_books_returns_zero_for_unknown_library() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        assert_eq!(count_books(&pool, "/nope").await.unwrap(), 0);
    }

    #[tokio::test]
    async fn library_from_db_with_total_reports_truncation() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let total = MAX_BOOKS_RETURNED + 7;
        seed_minimal_books(&pool, total).await;

        let (lib, returned_total) = library_from_db_with_total(&pool, Some("/lib"))
            .await
            .unwrap();
        assert_eq!(lib.path.as_deref(), Some("/lib"));
        assert_eq!(lib.books.len() as i64, MAX_BOOKS_RETURNED);
        assert_eq!(returned_total, total);
        assert!(
            returned_total > MAX_BOOKS_RETURNED,
            "test must seed strictly more rows than the cap"
        );
    }

    #[tokio::test]
    async fn library_from_db_with_total_reports_zero_for_none_path() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let (lib, total) = library_from_db_with_total(&pool, None).await.unwrap();
        assert!(lib.path.is_none());
        assert!(lib.books.is_empty());
        assert_eq!(total, 0);
    }

    #[tokio::test]
    async fn search_books_finds_by_title_and_ranks_by_bm25() {
        let _covers = CoversTempDir::new("fts_title");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed(
                    "a.epub",
                    Some("Harry Potter"),
                    &["J.K. Rowling"],
                    &[],
                    None,
                    None,
                ),
                indexed(
                    "b.epub",
                    Some("Something Else"),
                    &["Author B"],
                    &["harry"],
                    None,
                    None,
                ),
            ],
        )
        .await
        .unwrap();

        let hits = search_books(&pool, "/lib", "harry").await.unwrap();
        // Column filter scopes MATCH to title/authors/series — the tag-only
        // hit on "Something Else" is intentionally excluded.
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title.as_deref(), Some("Harry Potter"));
    }

    #[tokio::test]
    async fn search_books_finds_by_author_and_scopes_to_library() {
        let _covers = CoversTempDir::new("fts_author");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib-a",
            vec![indexed("a.epub", Some("A"), &["Tolkien"], &[], None, None)],
        )
        .await
        .unwrap();
        replace_books(
            &pool,
            "/lib-b",
            vec![indexed("b.epub", Some("B"), &["Tolkien"], &[], None, None)],
        )
        .await
        .unwrap();

        let hits = search_books(&pool, "/lib-a", "tolkien").await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title.as_deref(), Some("A"));
    }

    #[tokio::test]
    async fn search_books_empty_query_returns_empty_vec() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let hits = search_books(&pool, "/lib", "   ").await.unwrap();
        assert!(hits.is_empty());
    }

    #[tokio::test]
    async fn search_books_handles_unbalanced_quote_without_error() {
        let _covers = CoversTempDir::new("fts_unbalanced");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "a.epub",
                Some("Quoted"),
                &["Author"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        // Unbalanced `"` in raw input must not surface as a MATCH parse error.
        let hits = search_books(&pool, "/lib", "say \"hi")
            .await
            .expect("sanitizer guards against MATCH parse errors");
        assert!(hits.is_empty());
    }

    #[tokio::test]
    async fn search_books_excludes_isbn_column_from_match() {
        // ISBN is indexed in books_fts but the search column filter scopes
        // MATCH to title/authors/series, so ISBN lookups are intentionally
        // not surfaced. When/if we re-enable ISBN search, flip this to
        // assert a hit — no migration required.
        let _covers = CoversTempDir::new("fts_isbn");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let mut meta = indexed("a.epub", Some("ISBN Book"), &["A"], &[], None, None).metadata;
        meta.identifiers.push(Identifier {
            value: "978-0-123456-78-9".into(),
            scheme: Some("isbn".into()),
        });
        replace_books(
            &pool,
            "/lib",
            vec![IndexedBook {
                metadata: meta,
                cover: None,
                mtime_epoch: 0,
                size_bytes: 0,
            }],
        )
        .await
        .unwrap();

        let hits = search_books(&pool, "/lib", "978-0-123456-78-9")
            .await
            .unwrap();
        assert!(hits.is_empty());
    }

    #[tokio::test]
    async fn search_books_author_facet_filters_to_matching_author() {
        let _covers = CoversTempDir::new("fts_facet_author");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed(
                    "a.epub",
                    Some("Pride and Prejudice"),
                    &["Austen"],
                    &[],
                    None,
                    None,
                ),
                indexed("b.epub", Some("Hamlet"), &["Shakespeare"], &[], None, None),
            ],
        )
        .await
        .unwrap();

        let hits = search_books(&pool, "/lib", "author:austen").await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title.as_deref(), Some("Pride and Prejudice"));
    }

    #[tokio::test]
    async fn search_books_series_facet_filters_to_matching_series() {
        let _covers = CoversTempDir::new("fts_facet_series");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed(
                    "a.epub",
                    Some("Dune"),
                    &["Herbert"],
                    &[],
                    Some(("Dune Saga", "1")),
                    None,
                ),
                indexed("b.epub", Some("Standalone"), &["Author"], &[], None, None),
            ],
        )
        .await
        .unwrap();

        let hits = search_books(&pool, "/lib", "series:dune").await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title.as_deref(), Some("Dune"));
    }

    #[tokio::test]
    async fn search_books_tag_facet_filters_to_matching_tag() {
        let _covers = CoversTempDir::new("fts_facet_tag");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed("a.epub", Some("A"), &["X"], &["fiction"], None, None),
                indexed("b.epub", Some("B"), &["Y"], &["history"], None, None),
            ],
        )
        .await
        .unwrap();

        let hits = search_books(&pool, "/lib", "tag:fiction").await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title.as_deref(), Some("A"));
    }

    #[tokio::test]
    async fn search_books_facet_combines_with_free_text_via_explicit_and() {
        let _covers = CoversTempDir::new("fts_facet_combined");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed(
                    "a.epub",
                    Some("Pride and Prejudice"),
                    &["Austen"],
                    &[],
                    None,
                    None,
                ),
                indexed("b.epub", Some("Emma"), &["Austen"], &[], None, None),
            ],
        )
        .await
        .unwrap();

        // Both clauses must match — only Pride and Prejudice carries the
        // "pride" token in title/authors/series.
        let hits = search_books(&pool, "/lib", "author:austen pride")
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title.as_deref(), Some("Pride and Prejudice"));
    }

    #[tokio::test]
    async fn rename_author_updates_fts_via_trigger() {
        let _covers = CoversTempDir::new("fts_rename");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "a.epub",
                Some("Book"),
                &["OldName"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        assert_eq!(
            search_books(&pool, "/lib", "OldName").await.unwrap().len(),
            1
        );

        sqlx::query("UPDATE authors SET name = 'NewName' WHERE name = 'OldName'")
            .execute(&pool)
            .await
            .unwrap();

        assert_eq!(
            search_books(&pool, "/lib", "NewName").await.unwrap().len(),
            1
        );
        assert_eq!(
            search_books(&pool, "/lib", "OldName").await.unwrap().len(),
            0
        );
    }

    #[tokio::test]
    async fn reindex_keeps_fts_row_count_in_sync() {
        let _covers = CoversTempDir::new("fts_reindex");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed("a.epub", Some("A"), &["X"], &[], None, None),
                indexed("b.epub", Some("B"), &["Y"], &[], None, None),
            ],
        )
        .await
        .unwrap();
        // Reindex with one fewer book.
        replace_books(
            &pool,
            "/lib",
            vec![indexed("a.epub", Some("A"), &["X"], &[], None, None)],
        )
        .await
        .unwrap();

        let book_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books")
            .fetch_one(&pool)
            .await
            .unwrap();
        let fts_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books_fts")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(book_count, 1);
        assert_eq!(fts_count, 1, "FTS row count must match book count");
    }

    #[tokio::test]
    async fn list_books_populates_formats_from_book_files() {
        // Regression: F1.7 power-user table & inline format chips read
        // `EbookMetadata.formats` off the list endpoint; if list_books
        // returned `vec![]` the chip row would hide itself entirely.
        let _covers = CoversTempDir::new("list_books_formats");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "alpha.epub",
                Some("Alpha"),
                &["Author A"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();
        let books = list_books(&pool, "/lib").await.unwrap();
        assert_eq!(books.len(), 1);
        assert_eq!(books[0].formats, vec!["EPUB".to_string()]);
    }

    #[tokio::test]
    async fn list_books_returns_one_row_per_book_with_multi_format() {
        // Regression for PR #74 review: adding a second physical file
        // (EPUB + M4B) used to duplicate the parent row because the outer
        // query LEFT-JOINed `book_files`. The chip facets / table would
        // then over-count. Both queries now use scalar subqueries so the
        // result is one row per `books.id` and `.formats` carries both.
        let _covers = CoversTempDir::new("list_books_multi");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "alpha.epub",
                Some("Alpha"),
                &["Author A"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();
        let id = list_books(&pool, "/lib").await.unwrap()[0].id;
        sqlx::query(
            "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime)
             VALUES (?, 'M4B', 'alpha', 0, '')",
        )
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();

        let books = list_books(&pool, "/lib").await.unwrap();
        assert_eq!(books.len(), 1, "multi-format must not duplicate rows");
        assert_eq!(
            books[0].formats,
            vec!["EPUB".to_string(), "M4B".to_string()]
        );
        // EPUB wins the primary-filename tiebreak, matching get_book.
        assert_eq!(books[0].filename, "alpha.epub");
    }

    #[tokio::test]
    async fn search_books_returns_one_row_per_book_with_multi_format() {
        // Same regression in the FTS path.
        let _covers = CoversTempDir::new("search_books_multi");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "alpha.epub",
                Some("Alpha"),
                &["Author A"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();
        let id = list_books(&pool, "/lib").await.unwrap()[0].id;
        sqlx::query(
            "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime)
             VALUES (?, 'M4B', 'alpha', 0, '')",
        )
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();

        let results = search_books(&pool, "/lib", "Alpha").await.unwrap();
        assert_eq!(results.len(), 1, "FTS results must not duplicate rows");
        assert_eq!(
            results[0].formats,
            vec!["EPUB".to_string(), "M4B".to_string()]
        );
    }

    #[tokio::test]
    async fn list_books_filters_by_author_join() {
        let _covers = CoversTempDir::new("filter_author");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed("a.epub", Some("A"), &["Tolkien"], &[], None, None),
                indexed("b.epub", Some("B"), &["Pratchett"], &[], None, None),
            ],
        )
        .await
        .unwrap();
        let titles: Vec<String> = sqlx::query_scalar(
            "SELECT b.title FROM books b
             JOIN books_authors_link bal ON bal.book = b.id
             JOIN authors a ON a.id = bal.author
             WHERE a.name = ?",
        )
        .bind("Tolkien")
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(titles, vec!["A".to_string()]);
    }

    /// Regression guard for the N+1 fix in list_books / search_books.
    /// Both functions must return all creators, subjects, and identifiers
    /// via the inline json_group_array subqueries rather than per-book
    /// round-trip calls.
    #[tokio::test]
    async fn list_and_search_books_return_multi_valued_fields() {
        let _covers = CoversTempDir::new("multi_valued");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![IndexedBook {
                metadata: EbookMetadata {
                    filename: "multi.epub".into(),
                    title: Some("Multi".into()),
                    creators: vec![
                        Contributor {
                            name: "Alice".into(),
                            ..Default::default()
                        },
                        Contributor {
                            name: "Bob".into(),
                            ..Default::default()
                        },
                    ],
                    subjects: vec!["Fiction".into(), "Sci-Fi".into()],
                    identifiers: vec![
                        Identifier {
                            value: "978-0-000000-00-0".into(),
                            scheme: Some("isbn".into()),
                        },
                        Identifier {
                            value: "https://example.com/book/1".into(),
                            scheme: Some("uri".into()),
                        },
                    ],
                    ..Default::default()
                },
                cover: None,
                mtime_epoch: 0,
                size_bytes: 0,
            }],
        )
        .await
        .unwrap();

        let books = list_books(&pool, "/lib").await.unwrap();
        assert_eq!(books.len(), 1);
        let book = &books[0];
        assert_eq!(
            book.creators
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Alice", "Bob"]
        );
        assert_eq!(
            book.subjects,
            vec!["Fiction".to_string(), "Sci-Fi".to_string()]
        );
        assert_eq!(book.identifiers.len(), 2);
        assert_eq!(book.identifiers[0].scheme.as_deref(), Some("isbn"));
        assert_eq!(book.identifiers[1].scheme.as_deref(), Some("uri"));

        let hits = search_books(&pool, "/lib", "Multi").await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(
            hits[0]
                .creators
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Alice", "Bob"]
        );
        assert_eq!(
            hits[0].subjects,
            vec!["Fiction".to_string(), "Sci-Fi".to_string()]
        );
        assert_eq!(hits[0].identifiers.len(), 2);
    }

    #[tokio::test]
    async fn set_settings_prunes_library_when_ebook_path_changes() {
        let _covers = CoversTempDir::new("prune-change");
        let pool = init_db("sqlite::memory:").await.unwrap();
        set_settings(
            &pool,
            &Settings {
                ebook_library_path: Some("/old".into()),
                audiobook_library_path: None,
            },
        )
        .await
        .unwrap();
        replace_books(
            &pool,
            "/old",
            vec![indexed(
                "a.epub",
                Some("Dracula"),
                &["Stoker"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();
        assert_eq!(list_books(&pool, "/old").await.unwrap().len(), 1);

        set_settings(
            &pool,
            &Settings {
                ebook_library_path: Some("/new".into()),
                audiobook_library_path: None,
            },
        )
        .await
        .unwrap();

        assert!(list_books(&pool, "/old").await.unwrap().is_empty());
        let library_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM libraries")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(library_count, 0);
        let book_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(book_count, 0);
        let fts_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books_fts")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(fts_count, 0);
    }

    #[tokio::test]
    async fn set_settings_keeps_libraries_still_configured() {
        let _covers = CoversTempDir::new("prune-keep");
        let pool = init_db("sqlite::memory:").await.unwrap();
        set_settings(
            &pool,
            &Settings {
                ebook_library_path: Some("/books".into()),
                audiobook_library_path: Some("/audio".into()),
            },
        )
        .await
        .unwrap();
        replace_books(
            &pool,
            "/books",
            vec![indexed("a.epub", Some("A"), &["X"], &[], None, None)],
        )
        .await
        .unwrap();

        set_settings(
            &pool,
            &Settings {
                ebook_library_path: Some("/books".into()),
                audiobook_library_path: Some("/audio".into()),
            },
        )
        .await
        .unwrap();

        assert_eq!(list_books(&pool, "/books").await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn set_settings_none_removes_library_data() {
        let _covers = CoversTempDir::new("prune-clear");
        let pool = init_db("sqlite::memory:").await.unwrap();
        set_settings(
            &pool,
            &Settings {
                ebook_library_path: Some("/books".into()),
                audiobook_library_path: None,
            },
        )
        .await
        .unwrap();
        replace_books(
            &pool,
            "/books",
            vec![indexed("a.epub", Some("A"), &["X"], &[], None, None)],
        )
        .await
        .unwrap();

        set_settings(
            &pool,
            &Settings {
                ebook_library_path: None,
                audiobook_library_path: None,
            },
        )
        .await
        .unwrap();

        let library_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM libraries")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(library_count, 0);
    }

    /// Exercises the batched (#149) `prune_orphan_libraries` path across more
    /// than one chunk. The IN-list is chunked at 500 ids to stay under
    /// SQLite's bind-parameter cap, so seeding more orphaned libraries than a
    /// single chunk holds is what actually verifies the chunking loop iterates
    /// (a regression that dropped or mis-bound a later chunk would otherwise go
    /// undetected). Seeds 1001 libraries — three chunks of 500 / 500 / 1 — each
    /// with a book, then prunes them all in one transaction and asserts every
    /// cover UUID is collected (so the lookup is verified to span all chunks)
    /// and every row is gone. Only a handful of rows get an on-disk cover:
    /// materializing 1001 files would be slow, and cover deletion is exercised
    /// by the tracked subset, which straddles the chunk boundaries.
    #[tokio::test]
    async fn prune_orphan_libraries_batches_across_many_libraries() {
        // > 2 full chunks of 500 → the chunk loop runs three times.
        const LIBRARY_COUNT: usize = 1001;
        // Indices spanning every chunk: first row, the first row of the second
        // chunk, a mid-chunk row, and the final (third-chunk) row.
        const MATERIALIZED_COVER_INDICES: [usize; 4] = [0, 500, 750, LIBRARY_COUNT - 1];

        let _covers = CoversTempDir::new("prune-batch");
        let pool = init_db("sqlite::memory:").await.unwrap();

        // Seed the orphaned libraries directly. `keep = []` below marks all of
        // them as orphans regardless of path.
        let mut expected_uuids: Vec<String> = Vec::with_capacity(LIBRARY_COUNT);
        let mut materialized_uuids: Vec<String> = Vec::new();
        for i in 0..LIBRARY_COUNT {
            let path = format!("/orphan-{i}");
            let library_id: i64 = sqlx::query_scalar(
                "INSERT INTO libraries (path, display_name) VALUES (?, ?) RETURNING id",
            )
            .bind(&path)
            .bind(format!("Orphan {i}"))
            .fetch_one(&pool)
            .await
            .unwrap();

            let uuid = format!("uuid-{i}");
            sqlx::query(
                "INSERT INTO books (uuid, library_id, path, title, has_cover)
                 VALUES (?, ?, ?, ?, 1)",
            )
            .bind(&uuid)
            .bind(library_id)
            .bind(format!("{path}/book.epub"))
            .bind(format!("Book {i}"))
            .execute(&pool)
            .await
            .unwrap();

            // Materialize an on-disk cover for a few rows spanning the chunk
            // boundaries so deletion is observable without writing 1001 files.
            if MATERIALIZED_COVER_INDICES.contains(&i) {
                write_cover_file(&uuid, "image/jpeg", b"fake-jpeg").unwrap();
                assert!(cover_path_for(&uuid, "jpg").exists());
                materialized_uuids.push(uuid.clone());
            }

            expected_uuids.push(uuid);
        }

        let mut tx = pool.begin().await.unwrap();
        let mut orphan_uuids = prune_orphan_libraries(&mut tx, &[]).await.unwrap();
        tx.commit().await.unwrap();

        orphan_uuids.sort();
        expected_uuids.sort();
        assert_eq!(
            orphan_uuids, expected_uuids,
            "every orphaned book's cover UUID should be collected across all chunks"
        );

        let library_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM libraries")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(library_count, 0);
        let book_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(book_count, 0);

        // The caller deletes cover files post-commit; verify the collected
        // UUIDs drive removal of every materialized cover (one per chunk).
        delete_cover_files_for(&orphan_uuids);
        for uuid in &materialized_uuids {
            assert!(
                !cover_path_for(uuid, "jpg").exists(),
                "cover for {uuid} should be deleted"
            );
        }
    }

    #[tokio::test]
    async fn get_book_returns_none_for_missing_id() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let result = get_book(&pool, 9999).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn get_book_is_deterministic_with_multiple_files_and_links() {
        // Regression for PR #55 review: when a book has multiple `book_files`
        // rows (and incidental duplicate publisher/language/series links),
        // get_book() must return the EPUB-preferred filename and stable
        // publisher/language/series values rather than whichever joined row
        // SQLite happens to return first.
        let _covers = CoversTempDir::new("get_book_multi");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "alpha.epub",
                Some("Alpha Book"),
                &["Author A"],
                &["Fiction"],
                Some(("Saga", "1")),
                None,
            )],
        )
        .await
        .unwrap();
        let books = list_books(&pool, "/lib").await.unwrap();
        let id = books[0].id;

        // Add a second physical file in another format.
        sqlx::query(
            "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime)
             VALUES (?, 'M4B', 'alpha', 0, '')",
        )
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();

        // Add a second publisher and a second language to exercise the
        // multi-row JOIN path on those link tables. Series already has one
        // row from `replace_books`.
        sqlx::query("INSERT INTO publishers (name) VALUES ('Acme'), ('Zenith')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO books_publishers_link (book, publisher)
             SELECT ?, id FROM publishers WHERE name IN ('Acme', 'Zenith')",
        )
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO languages (code) VALUES ('eng'), ('fra')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO books_languages_link (book, language)
             SELECT ?, id FROM languages WHERE code IN ('eng', 'fra')",
        )
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();

        // Run get_book repeatedly — every call must return identical values.
        let first = get_book(&pool, id).await.unwrap().expect("book");
        for _ in 0..3 {
            let again = get_book(&pool, id).await.unwrap().expect("book");
            assert_eq!(again.filename, first.filename);
            assert_eq!(again.publisher, first.publisher);
            assert_eq!(again.language, first.language);
            assert_eq!(again.series, first.series);
            assert_eq!(again.formats, first.formats);
        }

        // EPUB should win the tiebreak for filename.
        assert_eq!(first.filename, "alpha.epub");
        // Both formats surface in the formats list, sorted by format code.
        assert_eq!(first.formats, vec!["EPUB".to_string(), "M4B".to_string()]);
        // Publisher/language pick alphabetical winners deterministically.
        assert_eq!(first.publisher.as_deref(), Some("Acme"));
        assert_eq!(first.language.as_deref(), Some("eng"));
        assert_eq!(first.series.as_deref(), Some("Saga"));
        assert_eq!(first.series_index.as_deref(), Some("1"));
    }

    #[tokio::test]
    async fn get_book_returns_metadata_for_indexed_book() {
        let _covers = CoversTempDir::new("get_book");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "alpha.epub",
                Some("Alpha Book"),
                &["Author A"],
                &["Fiction"],
                None,
                None,
            )],
        )
        .await
        .unwrap();
        let books = list_books(&pool, "/lib").await.unwrap();
        let id = books[0].id;

        let book = get_book(&pool, id)
            .await
            .unwrap()
            .expect("book should exist");
        assert_eq!(book.id, id);
        assert_eq!(book.title.as_deref(), Some("Alpha Book"));
        assert_eq!(book.creators.len(), 1);
        assert_eq!(book.creators[0].name, "Author A");
        assert_eq!(book.subjects, vec!["Fiction"]);
        assert!(!book.formats.is_empty(), "formats should be populated");
        assert!(
            book.formats.iter().any(|f| f.eq_ignore_ascii_case("epub")),
            "EPUB format should be present"
        );
    }

    #[tokio::test]
    async fn get_book_handles_book_with_no_relations() {
        // A `books` row that has zero m2m link rows and zero files: every
        // `json_group_array` subquery returns "[]" (over zero inner rows) and
        // every scalar subquery returns NULL. The function must still return
        // a populated `EbookMetadata` with empty vecs and an empty filename
        // rather than erroring out on the missing data.
        let pool = init_db("sqlite::memory:").await.unwrap();
        let lib_res =
            sqlx::query("INSERT INTO libraries (path, display_name) VALUES ('/lib', 'lib')")
                .execute(&pool)
                .await
                .unwrap();
        let lib_id = lib_res.last_insert_rowid();
        let res = sqlx::query(
            "INSERT INTO books (uuid, library_id, path, title) \
             VALUES ('lonely-uuid', ?, '/lib/lonely', 'Lonely')",
        )
        .bind(lib_id)
        .execute(&pool)
        .await
        .unwrap();
        let id = res.last_insert_rowid();

        let book = get_book(&pool, id)
            .await
            .unwrap()
            .expect("book should exist");
        assert_eq!(book.id, id);
        assert_eq!(book.title.as_deref(), Some("Lonely"));
        assert_eq!(book.filename, "");
        assert!(book.creators.is_empty());
        assert!(book.subjects.is_empty());
        assert!(book.identifiers.is_empty());
        assert!(book.formats.is_empty());
        assert!(book.publisher.is_none());
        assert!(book.language.is_none());
        assert!(book.series.is_none());
        assert!(book.cover_url.is_none());
    }

    #[tokio::test]
    async fn get_book_round_trips_values_containing_control_chars_and_quotes() {
        // Regression for PR #65 review: prior delimiter-based encoding
        // (`GROUP_CONCAT` with 0x1F/0x1E separators) would have silently
        // corrupted any value containing those control chars. The JSON
        // encoding must survive arbitrary UTF-8 — control chars, quotes,
        // backslashes, commas — without altering the round-tripped value.
        let _covers = CoversTempDir::new("get_book_collide");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let nasty_name = "Smith\u{001F}, John \"O'Reilly\" \\back\u{001E}/";
        let nasty_tag = "Sci-Fi\u{001F}Drama";
        let nasty_value = "9780\u{001E}123\"456\\";
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "alpha.epub",
                Some("Alpha"),
                &[nasty_name],
                &[nasty_tag],
                None,
                None,
            )],
        )
        .await
        .unwrap();
        let id = list_books(&pool, "/lib").await.unwrap()[0].id;
        sqlx::query("INSERT INTO book_identifiers (book_id, scheme, value) VALUES (?, 'ISBN', ?)")
            .bind(id)
            .bind(nasty_value)
            .execute(&pool)
            .await
            .unwrap();

        let book = get_book(&pool, id).await.unwrap().expect("book");
        assert_eq!(book.creators.len(), 1);
        assert_eq!(book.creators[0].name, nasty_name);
        assert_eq!(book.subjects, vec![nasty_tag.to_string()]);
        assert_eq!(book.identifiers.len(), 1);
        assert_eq!(book.identifiers[0].value, nasty_value);
        assert_eq!(book.identifiers[0].scheme.as_deref(), Some("ISBN"));
    }

    #[test]
    fn sanitize_description_preserves_safe_html() {
        let cleaned = sanitize_description(Some(
            "<p>Hello <strong>world</strong>!</p><p>Second <em>line</em>.</p>".into(),
        ))
        .unwrap();
        assert!(cleaned.contains("<p>"));
        assert!(cleaned.contains("<strong>world</strong>"));
        assert!(cleaned.contains("<em>line</em>"));
    }

    #[test]
    fn sanitize_description_strips_scripts_and_event_handlers() {
        // ammonia's defaults must drop <script>, inline `onerror`, and
        // `javascript:` URLs. Anything that could execute on the detail page
        // when rendered via dangerous_inner_html is the threat model here.
        let cleaned = sanitize_description(Some(
            "<p>Safe</p><script>alert('xss')</script>\
             <img src=x onerror=\"alert(1)\"/>\
             <a href=\"javascript:alert(1)\">click</a>"
                .into(),
        ))
        .unwrap();
        assert!(!cleaned.contains("<script"));
        assert!(!cleaned.to_ascii_lowercase().contains("onerror"));
        assert!(!cleaned.to_ascii_lowercase().contains("javascript:"));
        assert!(cleaned.contains("<p>Safe</p>"));
    }

    #[test]
    fn sanitize_description_collapses_empty_input_to_none() {
        assert_eq!(sanitize_description(None), None);
        assert_eq!(sanitize_description(Some(String::new())), None);
        assert_eq!(sanitize_description(Some("   \n\t".into())), None);
        // A bare <script> with no other content sanitizes to "" and must
        // collapse to None so the UI hides the description block entirely.
        assert_eq!(
            sanitize_description(Some("<script>alert(1)</script>".into())),
            None
        );
    }

    #[tokio::test]
    async fn get_book_returns_sanitized_html_description() {
        let _covers = CoversTempDir::new("sanitize_desc");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let raw =
            "<p>Brief.</p><script>alert('xss')</script><p>More <b>detail</b>.</p>".to_string();
        replace_books(
            &pool,
            "/lib",
            vec![IndexedBook {
                metadata: EbookMetadata {
                    filename: "alpha.epub".into(),
                    title: Some("Alpha".into()),
                    description: Some(raw),
                    ..Default::default()
                },
                cover: None,
                mtime_epoch: 0,
                size_bytes: 0,
            }],
        )
        .await
        .unwrap();
        let id = list_books(&pool, "/lib").await.unwrap()[0].id;

        let desc = get_book(&pool, id).await.unwrap().unwrap().description;
        let desc = desc.expect("description should be present");
        assert!(desc.contains("<p>Brief.</p>"));
        assert!(desc.contains("<b>detail</b>"));
        assert!(!desc.contains("<script"));
    }

    // -------------------------------------------------------------------------
    // Discovery query tests (F1.8)
    // -------------------------------------------------------------------------

    /// Seed a small multi-author, multi-series, multi-tag fixture for the
    /// discovery query tests below. Returns the pool and a `CoversTempDir`
    /// guard the caller must keep alive for the lifetime of the test.
    async fn seed_discovery_fixture() -> (SqlitePool, CoversTempDir) {
        let guard = CoversTempDir::new("discovery");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                // Two-author book in Saga #1 with tag "fiction"
                indexed(
                    "saga1.epub",
                    Some("Saga: Book One"),
                    &["Ada Lovelace", "Grace Hopper"],
                    &["fiction", "classic"],
                    Some(("Saga", "1")),
                    None,
                ),
                // Sequel in Saga #2, same primary author + new tag
                indexed(
                    "saga2.epub",
                    Some("Saga: Book Two"),
                    &["Ada Lovelace"],
                    &["fiction"],
                    Some(("Saga", "2")),
                    None,
                ),
                // Standalone by Ada — no series
                indexed(
                    "standalone.epub",
                    Some("Standalone"),
                    &["Ada Lovelace"],
                    &["essay"],
                    None,
                    None,
                ),
                // Different-author, different-series book
                indexed(
                    "other.epub",
                    Some("Other Story"),
                    &["Niklaus Wirth"],
                    &["nonfiction"],
                    Some(("Pioneers", "1")),
                    None,
                ),
            ],
        )
        .await
        .unwrap();
        (pool, guard)
    }

    async fn author_id_by_name(pool: &SqlitePool, name: &str) -> i64 {
        sqlx::query_scalar::<_, i64>("SELECT id FROM authors WHERE name = ?")
            .bind(name)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    async fn series_id_by_name(pool: &SqlitePool, name: &str) -> i64 {
        sqlx::query_scalar::<_, i64>("SELECT id FROM series WHERE name = ?")
            .bind(name)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn get_author_returns_author_with_all_books_ordered_by_series_index() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let id = author_id_by_name(&pool, "Ada Lovelace").await;

        let author = get_author(&pool, id).await.unwrap().expect("author exists");

        assert_eq!(author.name, "Ada Lovelace");
        assert_eq!(author.book_count, 3);
        assert_eq!(author.books.len(), 3);

        // Series books come first, ordered by series_index ASC (NULLS LAST
        // means the standalone trails).
        let titles: Vec<_> = author
            .books
            .iter()
            .filter_map(|b| b.title.clone())
            .collect();
        assert_eq!(
            titles,
            vec![
                "Saga: Book One".to_string(),
                "Saga: Book Two".to_string(),
                "Standalone".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn get_author_populates_series_id_on_books() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let id = author_id_by_name(&pool, "Ada Lovelace").await;
        let expected_sid = series_id_by_name(&pool, "Saga").await;

        let author = get_author(&pool, id).await.unwrap().unwrap();
        for book in author.books.iter().filter(|b| b.series.is_some()) {
            assert_eq!(
                book.series_id,
                Some(expected_sid),
                "series book should carry series_id"
            );
        }
        let standalone = author
            .books
            .iter()
            .find(|b| b.series.is_none())
            .expect("standalone present");
        assert_eq!(standalone.series_id, None);
    }

    #[tokio::test]
    async fn get_author_returns_none_for_missing_id() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let missing = get_author(&pool, 999_999).await.unwrap();
        assert!(missing.is_none());
    }

    #[tokio::test]
    async fn get_series_returns_books_ordered_by_series_index() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let id = series_id_by_name(&pool, "Saga").await;

        let series = get_series(&pool, id).await.unwrap().expect("series exists");
        assert_eq!(series.name, "Saga");
        assert_eq!(series.book_count, 2);

        let titles: Vec<_> = series
            .books
            .iter()
            .filter_map(|b| b.title.clone())
            .collect();
        assert_eq!(
            titles,
            vec!["Saga: Book One".to_string(), "Saga: Book Two".to_string()]
        );
        // Each book should carry the parent series id back out so the
        // frontend can navigate cross-references without an extra lookup.
        for book in &series.books {
            assert_eq!(book.series_id, Some(id));
        }
    }

    #[tokio::test]
    async fn get_series_returns_none_for_missing_id() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let missing = get_series(&pool, 999_999).await.unwrap();
        assert!(missing.is_none());
    }

    // -------------------------------------------------------------------------
    // Discovery read caps (issue #150)
    //
    // `get_author` / `get_series` previously serialized every attributed book
    // in one payload. The fix is a hard `LIMIT MAX_DISCOVERY_BOOKS` on the
    // nested `books` vec plus an uncapped `book_count` so callers can detect
    // truncation as `book_count > books.len()`.
    // -------------------------------------------------------------------------

    /// Seed `count` minimal `books` rows under `/lib`, all linked to one
    /// author ("Prolific") and one series ("Mega"), via recursive CTEs.
    /// Bypasses `replace_books`/the indexer — the cap only depends on link
    /// rows existing — keeping the test fast even past the 1k cap. Returns
    /// `(author_id, series_id)`.
    async fn seed_books_for_one_author_and_series(pool: &SqlitePool, count: i64) -> (i64, i64) {
        sqlx::query("INSERT INTO libraries (path, display_name) VALUES ('/lib', 'lib')")
            .execute(pool)
            .await
            .unwrap();
        let lib_id: i64 = sqlx::query_scalar("SELECT id FROM libraries WHERE path = '/lib'")
            .fetch_one(pool)
            .await
            .unwrap();
        let author_id: i64 = sqlx::query_scalar(
            "INSERT INTO authors (name, sort) VALUES ('Prolific', 'Prolific') RETURNING id",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        let series_id: i64 = sqlx::query_scalar(
            "INSERT INTO series (name, sort) VALUES ('Mega', 'Mega') RETURNING id",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        sqlx::query(
            r#"
            WITH RECURSIVE n(i) AS (
                SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < ?
            )
            INSERT INTO books (uuid, library_id, path, title, sort, series_index)
            SELECT 'uuid-' || i, ?, '/lib/b' || i, 'Title ' || i,
                   'Title ' || printf('%010d', i), i
              FROM n
            "#,
        )
        .bind(count)
        .bind(lib_id)
        .execute(pool)
        .await
        .unwrap();
        // Link every seeded book to the author and the series.
        sqlx::query(
            "INSERT INTO books_authors_link (book, author, position)
             SELECT id, ?, 0 FROM books",
        )
        .bind(author_id)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO books_series_link (book, series)
             SELECT id, ? FROM books",
        )
        .bind(series_id)
        .execute(pool)
        .await
        .unwrap();
        (author_id, series_id)
    }

    #[tokio::test]
    async fn get_author_caps_books_at_max_discovery_books() {
        let _covers = CoversTempDir::new("author_cap");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let total = MAX_DISCOVERY_BOOKS + 25;
        let (author_id, _series_id) = seed_books_for_one_author_and_series(&pool, total).await;

        let author = get_author(&pool, author_id)
            .await
            .unwrap()
            .expect("author exists");
        assert_eq!(
            author.books.len() as i64,
            MAX_DISCOVERY_BOOKS,
            "get_author must cap the nested books vec at MAX_DISCOVERY_BOOKS"
        );
        assert_eq!(
            author.book_count as i64, total,
            "book_count must report the true (uncapped) shelf size"
        );
        assert!(
            author.book_count > author.books.len(),
            "truncation must be detectable as book_count > books.len()"
        );
    }

    #[tokio::test]
    async fn get_series_caps_books_at_max_discovery_books() {
        let _covers = CoversTempDir::new("series_cap");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let total = MAX_DISCOVERY_BOOKS + 25;
        let (_author_id, series_id) = seed_books_for_one_author_and_series(&pool, total).await;

        let series = get_series(&pool, series_id)
            .await
            .unwrap()
            .expect("series exists");
        assert_eq!(
            series.books.len() as i64,
            MAX_DISCOVERY_BOOKS,
            "get_series must cap the nested books vec at MAX_DISCOVERY_BOOKS"
        );
        assert_eq!(
            series.book_count as i64, total,
            "book_count must report the true (uncapped) series size"
        );
        assert!(
            series.book_count > series.books.len(),
            "truncation must be detectable as book_count > books.len()"
        );
    }

    #[tokio::test]
    async fn get_tag_cloud_returns_counts_ordered_by_count_then_name() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let tags = get_tag_cloud(&pool).await.unwrap();

        // Fixture has: fiction × 2, classic × 1, essay × 1, nonfiction × 1.
        // Order: cnt DESC, then name ASC.
        let names: Vec<_> = tags.iter().map(|t| t.name.clone()).collect();
        assert_eq!(
            names,
            vec![
                "fiction".to_string(),
                "classic".to_string(),
                "essay".to_string(),
                "nonfiction".to_string(),
            ]
        );
        assert_eq!(tags[0].count, 2);
        assert!(tags[1..].iter().all(|t| t.count == 1));
    }

    #[tokio::test]
    async fn get_tag_cloud_returns_empty_vec_when_no_tags() {
        let _guard = CoversTempDir::new("empty_tags");
        let pool = init_db("sqlite::memory:").await.unwrap();
        // No books, no tags.
        let tags = get_tag_cloud(&pool).await.unwrap();
        assert!(tags.is_empty());
    }

    #[tokio::test]
    async fn get_tag_cloud_counts_reflect_overrides() {
        // F5.1: per-tag counts in the cloud must follow the merged view.
        // Without the override-aware count, the cloud kept showing the
        // canonical totals — over-reporting tags the user had removed
        // from books and missing books whose tags were reassigned via
        // override.
        let _guard = CoversTempDir::new("tag_cloud_overrides");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        replace_books(
            &pool,
            "/lib",
            vec![
                indexed("a.epub", Some("A"), &["X"], &["fiction"], None, None),
                indexed("b.epub", Some("B"), &["X"], &["fiction"], None, None),
                indexed("c.epub", Some("C"), &["X"], &["essay"], None, None),
            ],
        )
        .await
        .unwrap();

        // Sanity: canonical counts before any overrides.
        let pre = get_tag_cloud(&pool).await.unwrap();
        let fiction_pre = pre
            .iter()
            .find(|t| t.name == "fiction")
            .expect("fiction present pre-override");
        assert_eq!(fiction_pre.count, 2);

        // Reassign a.epub: drop "fiction", add "essay".
        let books = list_books(&pool, "/lib").await.unwrap();
        let a = books.iter().find(|b| b.filename == "a.epub").unwrap();
        let uuid = a.unique_identifier.clone().unwrap();
        let ov = MetadataOverrides {
            subjects: Some(vec!["essay".into()]),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let post = get_tag_cloud(&pool).await.unwrap();
        let fiction = post
            .iter()
            .find(|t| t.name == "fiction")
            .expect("fiction still visible (canonical anchor remains on b.epub)");
        assert_eq!(
            fiction.count, 1,
            "fiction should drop a.epub after override, got {post:?}",
        );
        let essay = post
            .iter()
            .find(|t| t.name == "essay")
            .expect("essay present");
        assert_eq!(
            essay.count, 2,
            "essay should pick up override-tagged a.epub, got {post:?}",
        );
    }

    // -----------------------------------------------------------------
    // F1.12 index pages — list_authors / list_series
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn list_authors_returns_all_with_counts_and_alpha_order() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let authors = list_authors(&pool, "/lib").await.unwrap();

        // Three distinct authors: Ada Lovelace, Grace Hopper, Niklaus Wirth.
        let names: Vec<_> = authors.iter().map(|a| a.name.clone()).collect();
        assert_eq!(
            names,
            vec![
                "Ada Lovelace".to_string(),
                "Grace Hopper".to_string(),
                "Niklaus Wirth".to_string(),
            ],
            "expected NOCASE alphabetical order by sort/name"
        );

        // Book counts: Ada=3, Grace=1, Niklaus=1.
        let by_name: std::collections::HashMap<_, _> = authors
            .iter()
            .map(|a| (a.name.clone(), a.book_count))
            .collect();
        assert_eq!(by_name["Ada Lovelace"], 3);
        assert_eq!(by_name["Grace Hopper"], 1);
        assert_eq!(by_name["Niklaus Wirth"], 1);

        // IDs are populated so cards can route to /authors/:id.
        assert!(authors.iter().all(|a| a.id > 0));
    }

    #[tokio::test]
    async fn list_authors_scopes_to_library_path() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let authors = list_authors(&pool, "/no-such-library").await.unwrap();
        assert!(
            authors.is_empty(),
            "unknown library path must yield empty list"
        );
    }

    #[tokio::test]
    async fn list_authors_returns_empty_for_empty_library() {
        let _guard = CoversTempDir::new("empty_authors");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let authors = list_authors(&pool, "/lib").await.unwrap();
        assert!(authors.is_empty());
    }

    #[tokio::test]
    async fn list_series_returns_all_with_counts_and_alpha_order() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let series = list_series(&pool, "/lib").await.unwrap();

        // Two series: Pioneers, Saga (NOCASE alpha).
        let names: Vec<_> = series.iter().map(|s| s.name.clone()).collect();
        assert_eq!(names, vec!["Pioneers".to_string(), "Saga".to_string()]);

        let by_name: std::collections::HashMap<_, _> = series
            .iter()
            .map(|s| (s.name.clone(), s.book_count))
            .collect();
        assert_eq!(by_name["Saga"], 2);
        assert_eq!(by_name["Pioneers"], 1);
    }

    #[tokio::test]
    async fn list_series_populates_primary_author_from_first_book() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let series = list_series(&pool, "/lib").await.unwrap();

        let by_name: std::collections::HashMap<_, _> = series
            .iter()
            .map(|s| (s.name.clone(), s.primary_author.clone()))
            .collect();
        // Saga book one's first creator is "Ada Lovelace" (the two-author
        // book lists Ada first); Pioneers has Niklaus Wirth as sole author.
        assert_eq!(by_name["Saga"], Some("Ada Lovelace".to_string()));
        assert_eq!(by_name["Pioneers"], Some("Niklaus Wirth".to_string()));
    }

    #[tokio::test]
    async fn list_series_scopes_to_library_path() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let series = list_series(&pool, "/no-such-library").await.unwrap();
        assert!(series.is_empty());
    }

    // F5.1 — index-page counts must follow the same override overlay
    // applied to `/authors/:id` and `/series/:id` in PR #153. Without
    // these, an author whose books were reassigned through the edit
    // form still reports the canonical count on /authors, then
    // /authors/:id shows the corrected list — a visible inconsistency.

    #[tokio::test]
    async fn list_authors_book_count_follows_override_creators() {
        // Setup: Ada has 3 canonical books, Grace has 1 (saga1 lists
        // both, with Ada first). Override saga2 so its single creator
        // is Grace instead of Ada. Expected effective counts:
        //   Ada   → 2 (saga1 + standalone; saga2 reassigned away)
        //   Grace → 2 (saga1 still + saga2 from override)
        let (pool, _guard) = seed_discovery_fixture().await;
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;
        let books = list_books(&pool, "/lib").await.unwrap();
        let saga2 = books.iter().find(|b| b.filename == "saga2.epub").unwrap();
        let uuid = saga2.unique_identifier.clone().unwrap();

        let ov = MetadataOverrides {
            creators: Some(vec![Contributor {
                name: "Grace Hopper".into(),
                role: Some("aut".into()),
                file_as: None,
                id: None,
            }]),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let authors = list_authors(&pool, "/lib").await.unwrap();
        let by_name: std::collections::HashMap<_, _> = authors
            .iter()
            .map(|a| (a.name.clone(), a.book_count))
            .collect();
        assert_eq!(
            by_name.get("Ada Lovelace").copied(),
            Some(2),
            "Ada loses saga2 to the override",
        );
        assert_eq!(
            by_name.get("Grace Hopper").copied(),
            Some(2),
            "Grace picks up saga2 from the override",
        );
    }

    #[tokio::test]
    async fn list_authors_book_count_matches_canonical_creator_case_insensitively() {
        // `authors.name` is `UNIQUE COLLATE NOCASE`; an override that
        // differs only by case ("ada lovelace") still resolves to the
        // canonical "Ada Lovelace" row. Mirrors the NOCASE follow-up
        // applied to /author/:id (commit aca8a81b).
        let (pool, _guard) = seed_discovery_fixture().await;
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;
        let books = list_books(&pool, "/lib").await.unwrap();
        let other = books.iter().find(|b| b.filename == "other.epub").unwrap();
        let uuid = other.unique_identifier.clone().unwrap();

        let ov = MetadataOverrides {
            creators: Some(vec![Contributor {
                name: "ADA LOVELACE".into(),
                role: Some("aut".into()),
                file_as: None,
                id: None,
            }]),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let authors = list_authors(&pool, "/lib").await.unwrap();
        let ada = authors
            .iter()
            .find(|a| a.name == "Ada Lovelace")
            .expect("Ada present");
        assert_eq!(
            ada.book_count, 4,
            "case-mismatched override should still increment canonical Ada's count",
        );
    }

    #[tokio::test]
    async fn list_series_book_count_follows_override_series() {
        // Move the standalone book into Saga via override. Expected:
        // Saga's count goes from 2 → 3. (The canonical books_series_link
        // is untouched; the overlay surfaces the effective membership.)
        let (pool, _guard) = seed_discovery_fixture().await;
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;
        let books = list_books(&pool, "/lib").await.unwrap();
        let standalone = books
            .iter()
            .find(|b| b.filename == "standalone.epub")
            .unwrap();
        let uuid = standalone.unique_identifier.clone().unwrap();

        let ov = MetadataOverrides {
            series: Some("Saga".into()),
            series_index: Some("3".into()),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let series = list_series(&pool, "/lib").await.unwrap();
        let saga = series
            .iter()
            .find(|s| s.name == "Saga")
            .expect("Saga present");
        assert_eq!(saga.book_count, 3, "override should add standalone to Saga");
    }

    #[tokio::test]
    async fn list_series_primary_author_follows_override_creators() {
        // Saga's first book (saga1.epub by series_index) has canonical
        // creators [Ada Lovelace, Grace Hopper] — primary_author is
        // "Ada Lovelace". Override the first creator to "Margaret
        // Hamilton"; the index by-line should follow.
        let (pool, _guard) = seed_discovery_fixture().await;
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;
        let books = list_books(&pool, "/lib").await.unwrap();
        let saga1 = books.iter().find(|b| b.filename == "saga1.epub").unwrap();
        let uuid = saga1.unique_identifier.clone().unwrap();

        let ov = MetadataOverrides {
            creators: Some(vec![Contributor {
                name: "Margaret Hamilton".into(),
                role: Some("aut".into()),
                file_as: None,
                id: None,
            }]),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let series = list_series(&pool, "/lib").await.unwrap();
        let saga = series
            .iter()
            .find(|s| s.name == "Saga")
            .expect("Saga present");
        assert_eq!(
            saga.primary_author.as_deref(),
            Some("Margaret Hamilton"),
            "override creator drives the index by-line",
        );
    }

    // -----------------------------------------------------------------
    // F5.1 Metadata overrides
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn upsert_and_get_metadata_overrides_roundtrips() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        // Create a user for updated_by.
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        let ov = MetadataOverrides {
            title: Some("New Title".into()),
            description: Some("A new description".into()),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, "test-uuid-1", &ov, false, user_id)
            .await
            .unwrap();

        let (loaded, has_cover) = get_metadata_overrides(&pool, "test-uuid-1")
            .await
            .unwrap()
            .expect("overrides should exist");
        assert_eq!(loaded.title, Some("New Title".into()));
        assert_eq!(loaded.description, Some("A new description".into()));
        assert_eq!(loaded.publisher, None);
        assert!(!has_cover);
    }

    #[tokio::test]
    async fn merge_metadata_overrides_accumulates_fields_and_preserves_cover_flag() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        // Seed an existing override carrying a title AND a user-uploaded cover.
        let initial = MetadataOverrides {
            title: Some("First Title".into()),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, "merge-uuid", &initial, true, user_id)
            .await
            .unwrap();

        // A later edit touching only `description` must not clobber the title
        // (the incremental-edit contract the TOCTOU race nullified) and must
        // not reset the cover flag (the pre-#166 reset bug).
        let edit = MetadataOverrides {
            description: Some("Added later".into()),
            ..Default::default()
        };
        merge_metadata_overrides(&pool, "merge-uuid", &edit, user_id)
            .await
            .unwrap();

        let (loaded, has_cover) = get_metadata_overrides(&pool, "merge-uuid")
            .await
            .unwrap()
            .expect("overrides should exist");
        assert_eq!(
            loaded.title,
            Some("First Title".into()),
            "prior title must survive a description-only merge"
        );
        assert_eq!(loaded.description, Some("Added later".into()));
        assert!(
            has_cover,
            "has_cover_override must carry forward across a text-only merge"
        );
    }

    #[tokio::test]
    async fn merge_metadata_overrides_creates_row_when_absent() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;
        let edit = MetadataOverrides {
            title: Some("Fresh".into()),
            ..Default::default()
        };
        merge_metadata_overrides(&pool, "fresh-uuid", &edit, user_id)
            .await
            .unwrap();
        let (loaded, has_cover) = get_metadata_overrides(&pool, "fresh-uuid")
            .await
            .unwrap()
            .expect("overrides should exist");
        assert_eq!(loaded.title, Some("Fresh".into()));
        assert!(!has_cover, "a brand-new merged row has no cover override");
    }

    #[tokio::test]
    async fn get_metadata_overrides_returns_none_when_absent() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let result = get_metadata_overrides(&pool, "nonexistent-uuid")
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn delete_metadata_overrides_removes_row() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;
        let ov = MetadataOverrides {
            title: Some("Override".into()),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, "del-uuid", &ov, false, user_id)
            .await
            .unwrap();
        assert!(get_metadata_overrides(&pool, "del-uuid")
            .await
            .unwrap()
            .is_some());

        delete_metadata_overrides(&pool, "del-uuid").await.unwrap();
        assert!(get_metadata_overrides(&pool, "del-uuid")
            .await
            .unwrap()
            .is_none());
    }

    /// Bug #1: saving a title override must rebuild `books_fts` so search
    /// finds the new title and stops matching the original one.
    #[tokio::test]
    async fn upsert_metadata_overrides_rebuilds_fts_for_title() {
        let _covers = CoversTempDir::new("fts_override_title");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "a.epub",
                Some("Original Title"),
                &["Author A"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        // Sanity: search finds the original title.
        let hits = search_books(&pool, "/lib", "Original").await.unwrap();
        assert_eq!(hits.len(), 1);

        // Save an override that changes the title.
        let uuid = list_books(&pool, "/lib").await.unwrap()[0]
            .unique_identifier
            .clone()
            .unwrap();
        let ov = MetadataOverrides {
            title: Some("Brand New Title".into()),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        // Search now matches the overridden title and no longer the original.
        let new_hits = search_books(&pool, "/lib", "Brand").await.unwrap();
        assert_eq!(new_hits.len(), 1);
        assert_eq!(new_hits[0].title.as_deref(), Some("Brand New Title"));
        let old_hits = search_books(&pool, "/lib", "Original").await.unwrap();
        assert!(
            old_hits.is_empty(),
            "FTS still matches the pre-override title"
        );
    }

    /// Bug #1: the palette uses the same `books_fts` table, so the override
    /// rebuild must also surface there.
    #[tokio::test]
    async fn upsert_metadata_overrides_rebuilds_fts_for_palette() {
        let _covers = CoversTempDir::new("fts_override_palette");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "p.epub",
                Some("Scanned Title"),
                &["Author"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let uuid = list_books(&pool, "/lib").await.unwrap()[0]
            .unique_identifier
            .clone()
            .unwrap();
        let ov = MetadataOverrides {
            title: Some("Edited Palette Title".into()),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let palette = search_palette(&pool, "/lib", "Edited").await.unwrap();
        assert_eq!(palette.books.len(), 1);
    }

    /// Bug #1 follow-on: deleting the override should restore the FTS row
    /// to the canonical scanned values.
    #[tokio::test]
    async fn delete_metadata_overrides_restores_fts() {
        let _covers = CoversTempDir::new("fts_override_revert");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "r.epub",
                Some("Canonical Title"),
                &["Author"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let uuid = list_books(&pool, "/lib").await.unwrap()[0]
            .unique_identifier
            .clone()
            .unwrap();

        upsert_metadata_overrides(
            &pool,
            &uuid,
            &MetadataOverrides {
                title: Some("Temporary Override".into()),
                ..Default::default()
            },
            false,
            user_id,
        )
        .await
        .unwrap();
        assert_eq!(
            search_books(&pool, "/lib", "Temporary")
                .await
                .unwrap()
                .len(),
            1
        );

        delete_metadata_overrides(&pool, &uuid).await.unwrap();

        // FTS is back to the canonical title; the override token no longer
        // matches.
        assert_eq!(
            search_books(&pool, "/lib", "Canonical")
                .await
                .unwrap()
                .len(),
            1
        );
        assert!(search_books(&pool, "/lib", "Temporary")
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn get_book_merges_scalar_overrides() {
        let _covers = CoversTempDir::new("merge_scalar");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "merge.epub",
                Some("Original Title"),
                &["Author A"],
                &["fiction"],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let books = list_books(&pool, "/lib").await.unwrap();
        let book = &books[0];
        let uuid = book.unique_identifier.clone().unwrap();
        let id = book.id;

        // Save overrides.
        let ov = MetadataOverrides {
            title: Some("Edited Title".into()),
            publisher: Some("New Publisher".into()),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        // get_book should return merged values.
        let merged = get_book(&pool, id).await.unwrap().unwrap();
        assert_eq!(merged.title.as_deref(), Some("Edited Title"));
        assert_eq!(merged.publisher.as_deref(), Some("New Publisher"));
        assert!(merged.has_override);
        // Non-overridden fields unchanged.
        assert_eq!(merged.creators[0].name, "Author A");
    }

    #[tokio::test]
    async fn get_book_merges_creators_replaces_entirely() {
        let _covers = CoversTempDir::new("merge_creators");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "creators.epub",
                Some("Book"),
                &["Author A"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let books = list_books(&pool, "/lib").await.unwrap();
        let uuid = books[0].unique_identifier.clone().unwrap();
        let id = books[0].id;

        let ov = MetadataOverrides {
            creators: Some(vec![
                Contributor {
                    name: "Author B".into(),
                    ..Default::default()
                },
                Contributor {
                    name: "Author C".into(),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let merged = get_book(&pool, id).await.unwrap().unwrap();
        assert_eq!(merged.creators.len(), 2);
        assert_eq!(merged.creators[0].name, "Author B");
        assert_eq!(merged.creators[1].name, "Author C");
    }

    #[tokio::test]
    async fn get_book_merges_subjects_replaces_entirely() {
        let _covers = CoversTempDir::new("merge_subjects");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "subjects.epub",
                Some("Book"),
                &["Author"],
                &["fiction", "classic"],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let books = list_books(&pool, "/lib").await.unwrap();
        let uuid = books[0].unique_identifier.clone().unwrap();
        let id = books[0].id;

        let ov = MetadataOverrides {
            subjects: Some(vec!["sci-fi".into(), "adventure".into()]),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let merged = get_book(&pool, id).await.unwrap().unwrap();
        assert_eq!(merged.subjects, vec!["sci-fi", "adventure"]);
    }

    #[tokio::test]
    async fn get_book_backfills_creator_ids_after_override_replaces_authors() {
        // Override Contributors carry only a name, so a book whose author
        // list was edited through the metadata form would otherwise come
        // back with `creators[*].id == None`, rendering the breadcrumb's
        // author link as an unclickable span even when the `authors` row
        // exists. Verify get_book backfills the id by name. Mirrors the
        // user's report against book 268 (multi-author book where the
        // user removed all but one canonical author).
        let (pool, _guard) = seed_discovery_fixture().await;
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;
        let ada_id = author_id_by_name(&pool, "Ada Lovelace").await;

        // saga1.epub canonically has ["Ada Lovelace", "Grace Hopper"];
        // simulate the user dropping the second author through the edit
        // form. apply_overrides replaces creators wholesale, so the
        // override Contributor has id = None.
        let books = list_books(&pool, "/lib").await.unwrap();
        let saga_one = books.iter().find(|b| b.filename == "saga1.epub").unwrap();
        let uuid = saga_one.unique_identifier.clone().unwrap();
        let book_id = saga_one.id;

        let ov = MetadataOverrides {
            creators: Some(vec![Contributor {
                name: "Ada Lovelace".into(),
                role: Some("aut".into()),
                file_as: None,
                id: None,
            }]),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let merged = get_book(&pool, book_id).await.unwrap().unwrap();
        assert_eq!(merged.creators.len(), 1);
        assert_eq!(merged.creators[0].name, "Ada Lovelace");
        assert_eq!(
            merged.creators[0].id,
            Some(ada_id),
            "creator id must be backfilled so the breadcrumb renders as a Link",
        );
    }

    #[tokio::test]
    async fn get_book_backfills_creator_ids_case_insensitively() {
        // `authors.name` is `UNIQUE COLLATE NOCASE`, so a SQL `IN (...)`
        // lookup matches case-insensitively — but the returned row carries
        // the DB casing while the override carries the user-supplied
        // casing. The HashMap must normalise both sides to lowercase so
        // an override like "ada lovelace" still resolves to the canonical
        // "Ada Lovelace" id.
        let (pool, _guard) = seed_discovery_fixture().await;
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;
        let ada_id = author_id_by_name(&pool, "Ada Lovelace").await;

        let books = list_books(&pool, "/lib").await.unwrap();
        let saga_one = books.iter().find(|b| b.filename == "saga1.epub").unwrap();
        let uuid = saga_one.unique_identifier.clone().unwrap();
        let book_id = saga_one.id;

        let ov = MetadataOverrides {
            creators: Some(vec![Contributor {
                name: "ADA LOVELACE".into(),
                role: Some("aut".into()),
                file_as: None,
                id: None,
            }]),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let merged = get_book(&pool, book_id).await.unwrap().unwrap();
        assert_eq!(merged.creators.len(), 1);
        assert_eq!(merged.creators[0].name, "ADA LOVELACE");
        assert_eq!(
            merged.creators[0].id,
            Some(ada_id),
            "case-mismatched override should still resolve to the canonical author id",
        );
    }

    #[tokio::test]
    async fn get_book_leaves_creator_id_none_when_override_author_unknown() {
        // If the override sets an author name that doesn't exist in the
        // `authors` table, backfill must leave the id None — same shape
        // as get_book_leaves_series_id_none_when_override_series_unknown.
        let (pool, _guard) = seed_discovery_fixture().await;
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        let books = list_books(&pool, "/lib").await.unwrap();
        let saga_one = books.iter().find(|b| b.filename == "saga1.epub").unwrap();
        let uuid = saga_one.unique_identifier.clone().unwrap();
        let book_id = saga_one.id;

        let ov = MetadataOverrides {
            creators: Some(vec![Contributor {
                name: "Nobody Indexed".into(),
                role: Some("aut".into()),
                file_as: None,
                id: None,
            }]),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let merged = get_book(&pool, book_id).await.unwrap().unwrap();
        assert_eq!(merged.creators.len(), 1);
        assert_eq!(merged.creators[0].name, "Nobody Indexed");
        assert_eq!(merged.creators[0].id, None);
    }

    #[tokio::test]
    async fn get_book_backfills_series_id_from_override_when_series_exists() {
        // A book whose series was set via overrides (not at scan time)
        // historically came back with series_id == None even though the
        // series row existed in the relational table. The detail page's
        // "Series" rail then fell back to plain text instead of a Link
        // to /series/:id. Verify the read path now backfills the id.
        let _covers = CoversTempDir::new("override_series_link");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        // Seed: one book belongs to "Saga" natively (so the series row exists),
        // one standalone book that we'll later override into the same series.
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed(
                    "saga1.epub",
                    Some("Saga: Book One"),
                    &["Author X"],
                    &[],
                    Some(("Saga", "1")),
                    None,
                ),
                indexed("loner.epub", Some("Loner"), &["Author Y"], &[], None, None),
            ],
        )
        .await
        .unwrap();

        let saga_id = series_id_by_name(&pool, "Saga").await;
        let books = list_books(&pool, "/lib").await.unwrap();
        let loner = books.iter().find(|b| b.filename == "loner.epub").unwrap();
        assert_eq!(loner.series, None);
        assert_eq!(loner.series_id, None);
        let loner_uuid = loner.unique_identifier.clone().unwrap();
        let loner_book_id = loner.id;

        // Override the standalone to be part of "Saga". The overrides path
        // does not touch books_series_link, so loner.series_id stays unset
        // in the relational table — get_book must backfill from the series
        // table by name.
        let ov = MetadataOverrides {
            series: Some("Saga".into()),
            series_index: Some("3".into()),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &loner_uuid, &ov, false, user_id)
            .await
            .unwrap();

        let merged = get_book(&pool, loner_book_id).await.unwrap().unwrap();
        assert_eq!(merged.series.as_deref(), Some("Saga"));
        assert_eq!(
            merged.series_id,
            Some(saga_id),
            "override-only series must still resolve series_id so the detail rail can link"
        );
    }

    #[tokio::test]
    async fn get_author_includes_books_whose_override_names_this_author() {
        // Repro of the bug where renaming a book's author via the
        // metadata form (e.g. "Sanderson, Brandon" → "Brandon Sanderson")
        // left the book invisible on the new author's `/author/:id` page.
        // The override path writes JSON only — `books_authors_link` keeps
        // pointing at the canonical author row — so `get_author` must
        // layer overrides on top of the relational link at read time.
        let (pool, _guard) = seed_discovery_fixture().await;
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        // Set up the "Brandon Sanderson" vs "Sanderson, Brandon" shape:
        // one canonical author and a second name the user prefers, then
        // override one book to use the preferred name.
        let canonical_id = author_id_by_name(&pool, "Ada Lovelace").await;
        let preferred_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO authors (name, sort) VALUES (?, ?) RETURNING id",
        )
        .bind("Lovelace, Ada")
        .bind("Lovelace, Ada")
        .fetch_one(&pool)
        .await
        .unwrap();

        let books = list_books(&pool, "/lib").await.unwrap();
        let saga_one = books.iter().find(|b| b.filename == "saga1.epub").unwrap();
        let uuid = saga_one.unique_identifier.clone().unwrap();
        let saga_one_id = saga_one.id;

        // saga1.epub canonically lists ["Ada Lovelace", "Grace Hopper"];
        // the override renames the primary author to "Lovelace, Ada".
        let ov = MetadataOverrides {
            creators: Some(vec![
                Contributor {
                    name: "Lovelace, Ada".into(),
                    role: Some("aut".into()),
                    file_as: None,
                    id: None,
                },
                Contributor {
                    name: "Grace Hopper".into(),
                    role: Some("aut".into()),
                    file_as: None,
                    id: None,
                },
            ]),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        // Visiting the preferred-name author page must now include the
        // overridden book, even though `books_authors_link` for that book
        // still points at the canonical "Ada Lovelace" row.
        let preferred = get_author(&pool, preferred_id)
            .await
            .unwrap()
            .expect("author exists");
        let titles: Vec<_> = preferred
            .books
            .iter()
            .map(|b| b.title.clone().unwrap_or_default())
            .collect();
        assert_eq!(
            titles,
            vec!["Saga: Book One".to_string()],
            "override-named author must surface the book on /author/:id",
        );

        // And the canonical-name author page must drop it, because the
        // override replaced the creator list wholesale.
        let canonical = get_author(&pool, canonical_id)
            .await
            .unwrap()
            .expect("author exists");
        let canonical_titles: Vec<_> = canonical
            .books
            .iter()
            .map(|b| b.title.clone().unwrap_or_default())
            .collect();
        assert!(
            !canonical_titles.contains(&"Saga: Book One".to_string()),
            "override moved the book off the canonical author, got {canonical_titles:?}",
        );

        // The card on the preferred-name page should show the override
        // creator name, not the canonical one.
        let card = &preferred.books[0];
        assert_eq!(card.id, saga_one_id);
        assert_eq!(
            card.creators.first().map(|c| c.name.as_str()),
            Some("Lovelace, Ada")
        );
    }

    #[tokio::test]
    async fn get_author_excludes_books_whose_override_clears_authors() {
        // A book whose override sets creators to the empty array should
        // disappear from every author's page, matching what the book
        // detail page already shows (no breadcrumb author).
        let (pool, _guard) = seed_discovery_fixture().await;
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;
        let ada_id = author_id_by_name(&pool, "Ada Lovelace").await;

        let books = list_books(&pool, "/lib").await.unwrap();
        let standalone = books
            .iter()
            .find(|b| b.filename == "standalone.epub")
            .unwrap();
        let uuid = standalone.unique_identifier.clone().unwrap();

        let ov = MetadataOverrides {
            creators: Some(vec![]),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let ada = get_author(&pool, ada_id)
            .await
            .unwrap()
            .expect("author exists");
        let titles: Vec<_> = ada
            .books
            .iter()
            .map(|b| b.title.clone().unwrap_or_default())
            .collect();
        assert!(
            !titles.contains(&"Standalone".to_string()),
            "override-cleared creators must drop the book from /author/:id, got {titles:?}",
        );
    }

    #[tokio::test]
    async fn get_author_override_creator_match_is_case_insensitive() {
        // `authors.name` is `UNIQUE COLLATE NOCASE`, so an override that
        // differs only by case from the target author's row must still
        // surface the book on `/author/:id`. The override comparison
        // gets an explicit `COLLATE NOCASE` because the LHS is a
        // `json_extract(...)` expression (BINARY by default) and the RHS
        // is a bound parameter (also no collation).
        let (pool, _guard) = seed_discovery_fixture().await;
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;
        let ada_id = author_id_by_name(&pool, "Ada Lovelace").await;

        let books = list_books(&pool, "/lib").await.unwrap();
        let standalone = books
            .iter()
            .find(|b| b.filename == "standalone.epub")
            .unwrap();
        let uuid = standalone.unique_identifier.clone().unwrap();

        // Override uses lowercase casing; canonical row is "Ada Lovelace".
        let ov = MetadataOverrides {
            creators: Some(vec![Contributor {
                name: "ada lovelace".into(),
                role: Some("aut".into()),
                file_as: None,
                id: None,
            }]),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let ada = get_author(&pool, ada_id)
            .await
            .unwrap()
            .expect("author exists");
        let titles: Vec<_> = ada
            .books
            .iter()
            .map(|b| b.title.clone().unwrap_or_default())
            .collect();
        assert!(
            titles.contains(&"Standalone".to_string()),
            "lowercase override should still match NOCASE author row, got {titles:?}",
        );
    }

    #[tokio::test]
    async fn get_series_includes_books_added_via_override() {
        // Repro of the bug where editing a book to set its series via the
        // metadata form left the book invisible on `/series/:id`. The
        // override path only writes JSON into `metadata_overrides` and
        // never touches `books_series_link`, so `get_series` must layer
        // overrides on top of the relational link at read time.
        let (pool, _guard) = seed_discovery_fixture().await;
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;
        let saga_id = series_id_by_name(&pool, "Saga").await;

        // Loner has no canonical series at all. After the override it
        // should show up as #3 in Saga, after the two indexed books.
        let books = list_books(&pool, "/lib").await.unwrap();
        let standalone = books
            .iter()
            .find(|b| b.filename == "standalone.epub")
            .unwrap();
        let standalone_uuid = standalone.unique_identifier.clone().unwrap();
        let standalone_id = standalone.id;

        let ov = MetadataOverrides {
            series: Some("Saga".into()),
            series_index: Some("3".into()),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &standalone_uuid, &ov, false, user_id)
            .await
            .unwrap();

        let series = get_series(&pool, saga_id)
            .await
            .unwrap()
            .expect("series exists");
        assert_eq!(series.book_count, 3);

        let titles: Vec<_> = series
            .books
            .iter()
            .map(|b| b.title.clone().unwrap_or_default())
            .collect();
        assert_eq!(
            titles,
            vec![
                "Saga: Book One".to_string(),
                "Saga: Book Two".to_string(),
                "Standalone".to_string(),
            ],
            "override-set series_index=3 should sort the overridden book last",
        );

        // The overridden book must carry the parent series id so the card
        // links back to /series/:id.
        let overridden = series.books.iter().find(|b| b.id == standalone_id).unwrap();
        assert_eq!(overridden.series_id, Some(saga_id));
        assert_eq!(overridden.series.as_deref(), Some("Saga"));
    }

    #[tokio::test]
    async fn get_series_excludes_books_whose_override_clears_series() {
        // A book canonically in Saga whose override clears the series (sets
        // series to an empty string) should disappear from /series/:id,
        // matching what the book detail page already shows.
        let (pool, _guard) = seed_discovery_fixture().await;
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;
        let saga_id = series_id_by_name(&pool, "Saga").await;

        let books = list_books(&pool, "/lib").await.unwrap();
        let book_two = books.iter().find(|b| b.filename == "saga2.epub").unwrap();
        let uuid = book_two.unique_identifier.clone().unwrap();

        let ov = MetadataOverrides {
            series: Some(String::new()),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let series = get_series(&pool, saga_id)
            .await
            .unwrap()
            .expect("series exists");
        assert_eq!(series.book_count, 1);
        assert_eq!(
            series.books[0].title.as_deref(),
            Some("Saga: Book One"),
            "the unaffected book stays; the cleared one drops out",
        );
    }

    #[tokio::test]
    async fn get_series_override_match_is_case_insensitive() {
        // The CTE's `series_name` column is BINARY by default — without
        // `COLLATE NOCASE` on the filter, an override that differs only
        // by case from the canonical series row fails to match, even
        // though `series.name` is `UNIQUE COLLATE NOCASE`.
        let (pool, _guard) = seed_discovery_fixture().await;
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;
        let saga_id = series_id_by_name(&pool, "Saga").await;

        let books = list_books(&pool, "/lib").await.unwrap();
        let standalone = books
            .iter()
            .find(|b| b.filename == "standalone.epub")
            .unwrap();
        let uuid = standalone.unique_identifier.clone().unwrap();

        // Override uses lowercase casing; canonical row is "Saga".
        let ov = MetadataOverrides {
            series: Some("saga".into()),
            series_index: Some("3".into()),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let series = get_series(&pool, saga_id)
            .await
            .unwrap()
            .expect("series exists");
        let titles: Vec<_> = series
            .books
            .iter()
            .map(|b| b.title.clone().unwrap_or_default())
            .collect();
        assert!(
            titles.contains(&"Standalone".to_string()),
            "lowercase override should still match NOCASE series row, got {titles:?}",
        );
    }

    #[tokio::test]
    async fn get_author_empty_string_series_index_sorts_last() {
        // Mirror of get_series: clearing the position field (`Some("")`)
        // used to CAST('') to 0.0 in get_author's ORDER BY and sort the
        // book to the front of the author's shelf. NULLIF drops it to NULL
        // so NULLS LAST trails it behind positioned books.
        let (pool, _guard) = seed_discovery_fixture().await;
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;
        let author_id = author_id_by_name(&pool, "Ada Lovelace").await;

        let books = list_books(&pool, "/lib").await.unwrap();
        let book_one = books.iter().find(|b| b.filename == "saga1.epub").unwrap();
        let uuid = book_one.unique_identifier.clone().unwrap();

        // Keep Book One (canonical Saga #1) but clear its position.
        let ov = MetadataOverrides {
            series: Some("Saga".into()),
            series_index: Some(String::new()),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let author = get_author(&pool, author_id)
            .await
            .unwrap()
            .expect("author exists");
        let titles: Vec<_> = author
            .books
            .iter()
            .map(|b| b.title.clone().unwrap_or_default())
            .collect();
        let pos = |t: &str| titles.iter().position(|x| x == t).unwrap();
        assert!(
            pos("Saga: Book Two") < pos("Saga: Book One"),
            "cleared series_index should trail the positioned book, got {titles:?}",
        );
        assert_ne!(
            titles.first().map(String::as_str),
            Some("Saga: Book One"),
            "cleared series_index must not sort to the front, got {titles:?}",
        );
    }

    #[tokio::test]
    async fn get_series_empty_string_series_index_sorts_last() {
        // `Some("")` from the edit form (user cleared the position
        // field) was sorting to the front because `CAST('' AS REAL)`
        // returns 0.0. NULLIF on the override value drops it to NULL,
        // and ORDER BY ... NULLS LAST trails it after positioned books.
        let (pool, _guard) = seed_discovery_fixture().await;
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;
        let saga_id = series_id_by_name(&pool, "Saga").await;

        let books = list_books(&pool, "/lib").await.unwrap();
        let standalone = books
            .iter()
            .find(|b| b.filename == "standalone.epub")
            .unwrap();
        let uuid = standalone.unique_identifier.clone().unwrap();

        // Add Standalone to Saga but clear its position.
        let ov = MetadataOverrides {
            series: Some("Saga".into()),
            series_index: Some(String::new()),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let series = get_series(&pool, saga_id)
            .await
            .unwrap()
            .expect("series exists");
        let titles: Vec<_> = series
            .books
            .iter()
            .map(|b| b.title.clone().unwrap_or_default())
            .collect();
        assert_eq!(
            titles,
            vec![
                "Saga: Book One".to_string(),
                "Saga: Book Two".to_string(),
                "Standalone".to_string(),
            ],
            "empty-string series_index should trail positioned books, not lead them",
        );
    }

    #[tokio::test]
    async fn get_series_pins_series_id_for_books_moved_between_series() {
        // A book canonically in Series A overridden into Series B used
        // to come back from get_series(B) with `series_id = Some(A)`
        // (BOOK_COLUMNS reads only books_series_link), so the card on
        // B's page would link back to /series/A. The fix pins
        // series_id/series unconditionally to the requested parent.
        let (pool, _guard) = seed_discovery_fixture().await;
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;
        let pioneers_id = series_id_by_name(&pool, "Pioneers").await;

        // "Other Story" is canonically in Pioneers; override moves it
        // into Saga. Verify that opening Saga's page returns the book
        // pinned to Saga's id, not Pioneers'.
        let books = list_books(&pool, "/lib").await.unwrap();
        let other = books.iter().find(|b| b.filename == "other.epub").unwrap();
        let uuid = other.unique_identifier.clone().unwrap();

        let saga_id = series_id_by_name(&pool, "Saga").await;
        let ov = MetadataOverrides {
            series: Some("Saga".into()),
            series_index: Some("5".into()),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let saga = get_series(&pool, saga_id)
            .await
            .unwrap()
            .expect("Saga exists");
        let moved = saga
            .books
            .iter()
            .find(|b| b.title.as_deref() == Some("Other Story"))
            .expect("override moved Other Story into Saga");
        assert_eq!(
            moved.series_id,
            Some(saga_id),
            "card on Saga's page must link back to Saga, not the canonical Pioneers",
        );
        assert_eq!(moved.series.as_deref(), Some("Saga"));

        // And it should be gone from Pioneers' page.
        let pioneers = get_series(&pool, pioneers_id)
            .await
            .unwrap()
            .expect("Pioneers exists");
        assert!(
            !pioneers
                .books
                .iter()
                .any(|b| b.title.as_deref() == Some("Other Story")),
            "override moved Other Story off Pioneers",
        );
    }

    #[tokio::test]
    async fn get_book_leaves_series_id_none_when_override_series_unknown() {
        // If the override sets a series name that no other book uses, the
        // series table won't have a row to point at — backfill must
        // leave series_id None rather than fabricating one.
        let _covers = CoversTempDir::new("override_series_unknown");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "alone.epub",
                Some("Alone"),
                &["Author"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let books = list_books(&pool, "/lib").await.unwrap();
        let book = &books[0];
        let uuid = book.unique_identifier.clone().unwrap();
        let id = book.id;

        let ov = MetadataOverrides {
            series: Some("A Series That Does Not Yet Exist".into()),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let merged = get_book(&pool, id).await.unwrap().unwrap();
        assert_eq!(
            merged.series.as_deref(),
            Some("A Series That Does Not Yet Exist")
        );
        assert_eq!(merged.series_id, None);
    }

    #[tokio::test]
    async fn overrides_survive_reindex() {
        let _covers = CoversTempDir::new("reindex_survive");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        // First index.
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "survive.epub",
                Some("Original"),
                &["Author"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let books = list_books(&pool, "/lib").await.unwrap();
        let uuid = books[0].unique_identifier.clone().unwrap();

        // Save overrides.
        let ov = MetadataOverrides {
            title: Some("Overridden".into()),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        // Reindex — replace_books deletes and re-inserts.
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "survive.epub",
                Some("Original"),
                &["Author"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        // The new book row has a new id but the same UUID.
        let books = list_books(&pool, "/lib").await.unwrap();
        let id = books[0].id;
        let merged = get_book(&pool, id).await.unwrap().unwrap();
        assert_eq!(
            merged.title.as_deref(),
            Some("Overridden"),
            "overrides should survive the DELETE/INSERT reindex"
        );
        assert!(merged.has_override);
    }

    #[tokio::test]
    async fn list_books_merges_overrides_in_bulk() {
        let _covers = CoversTempDir::new("bulk_merge");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        replace_books(
            &pool,
            "/lib",
            vec![
                indexed("a.epub", Some("Book A"), &["Author A"], &[], None, None),
                indexed("b.epub", Some("Book B"), &["Author B"], &[], None, None),
                indexed("c.epub", Some("Book C"), &["Author C"], &[], None, None),
            ],
        )
        .await
        .unwrap();

        let books = list_books(&pool, "/lib").await.unwrap();
        let uuid_a = books
            .iter()
            .find(|b| b.title.as_deref() == Some("Book A"))
            .unwrap()
            .unique_identifier
            .clone()
            .unwrap();
        let uuid_c = books
            .iter()
            .find(|b| b.title.as_deref() == Some("Book C"))
            .unwrap()
            .unique_identifier
            .clone()
            .unwrap();

        // Override A and C only.
        upsert_metadata_overrides(
            &pool,
            &uuid_a,
            &MetadataOverrides {
                title: Some("Edited A".into()),
                ..Default::default()
            },
            false,
            user_id,
        )
        .await
        .unwrap();
        upsert_metadata_overrides(
            &pool,
            &uuid_c,
            &MetadataOverrides {
                title: Some("Edited C".into()),
                ..Default::default()
            },
            false,
            user_id,
        )
        .await
        .unwrap();

        let books = list_books(&pool, "/lib").await.unwrap();
        let a = books
            .iter()
            .find(|b| b.unique_identifier.as_deref() == Some(&uuid_a))
            .unwrap();
        let b = books
            .iter()
            .find(|b| b.title.as_deref() == Some("Book B"))
            .unwrap();
        let c = books
            .iter()
            .find(|b| b.unique_identifier.as_deref() == Some(&uuid_c))
            .unwrap();

        assert_eq!(a.title.as_deref(), Some("Edited A"));
        assert!(a.has_override);
        assert_eq!(b.title.as_deref(), Some("Book B"));
        assert!(!b.has_override);
        assert_eq!(c.title.as_deref(), Some("Edited C"));
        assert!(c.has_override);
    }

    #[tokio::test]
    async fn delete_overrides_reverts_to_scanned() {
        let _covers = CoversTempDir::new("revert");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "revert.epub",
                Some("Original"),
                &["Author"],
                &["fiction"],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let books = list_books(&pool, "/lib").await.unwrap();
        let uuid = books[0].unique_identifier.clone().unwrap();
        let id = books[0].id;

        // Override.
        upsert_metadata_overrides(
            &pool,
            &uuid,
            &MetadataOverrides {
                title: Some("Changed".into()),
                subjects: Some(vec!["sci-fi".into()]),
                ..Default::default()
            },
            false,
            user_id,
        )
        .await
        .unwrap();
        let merged = get_book(&pool, id).await.unwrap().unwrap();
        assert_eq!(merged.title.as_deref(), Some("Changed"));

        // Delete overrides — should revert to scanned.
        delete_metadata_overrides(&pool, &uuid).await.unwrap();
        let reverted = get_book(&pool, id).await.unwrap().unwrap();
        assert_eq!(reverted.title.as_deref(), Some("Original"));
        assert_eq!(reverted.subjects, vec!["fiction"]);
        assert!(!reverted.has_override);
    }

    /// Verify that `MetadataOverrides::merge` correctly layers a second edit
    /// on top of a first without losing the first edit's fields.
    #[tokio::test]
    async fn merge_preserves_prior_overrides() {
        let first = MetadataOverrides {
            title: Some("Edited Title".into()),
            publisher: Some("Edited Publisher".into()),
            ..Default::default()
        };
        let second = MetadataOverrides {
            description: Some("New description".into()),
            ..Default::default()
        };
        let merged = first.merge(&second);
        // second's description wins
        assert_eq!(merged.description.as_deref(), Some("New description"));
        // first's title and publisher are preserved (not wiped by None)
        assert_eq!(merged.title.as_deref(), Some("Edited Title"));
        assert_eq!(merged.publisher.as_deref(), Some("Edited Publisher"));
        // unset in both stays None
        assert_eq!(merged.language, None);
    }

    // ── search_palette ──────────────────────────────────────────────

    #[tokio::test]
    async fn palette_books_match_title() {
        let _covers = CoversTempDir::new("palette_books");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed(
                    "a.epub",
                    Some("Dracula"),
                    &["Bram Stoker"],
                    &["Horror"],
                    None,
                    None,
                ),
                indexed(
                    "b.epub",
                    Some("Frankenstein"),
                    &["Mary Shelley"],
                    &["Horror"],
                    None,
                    None,
                ),
            ],
        )
        .await
        .unwrap();

        let results = search_palette(&pool, "/lib", "dracula").await.unwrap();
        assert_eq!(results.books.len(), 1);
        assert_eq!(results.books[0].title, "Dracula");
        assert_eq!(results.books[0].author_display, "Bram Stoker");
        assert!(results.books[0].formats.contains(&"EPUB".to_string()));
    }

    #[tokio::test]
    async fn palette_authors_match_substring() {
        let _covers = CoversTempDir::new("palette_authors");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "a.epub",
                Some("Babel"),
                &["R. F. Kuang"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let results = search_palette(&pool, "/lib", "kuang").await.unwrap();
        assert!(!results.authors.is_empty(), "should match author substring");
        assert_eq!(results.authors[0].name, "R. F. Kuang");
        assert_eq!(results.authors[0].book_count, 1);
    }

    #[tokio::test]
    async fn palette_series_match() {
        let _covers = CoversTempDir::new("palette_series");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed(
                    "a.epub",
                    Some("Book One"),
                    &["Author"],
                    &[],
                    Some(("Poppy War", "1")),
                    None,
                ),
                indexed(
                    "b.epub",
                    Some("Book Two"),
                    &["Author"],
                    &[],
                    Some(("Poppy War", "2")),
                    None,
                ),
            ],
        )
        .await
        .unwrap();

        let results = search_palette(&pool, "/lib", "poppy").await.unwrap();
        assert!(!results.series.is_empty(), "should match series substring");
        assert_eq!(results.series[0].name, "Poppy War");
        assert_eq!(results.series[0].book_count, 2);
    }

    #[tokio::test]
    async fn palette_tags_match() {
        let _covers = CoversTempDir::new("palette_tags");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "a.epub",
                Some("A"),
                &["Author"],
                &["Dark academia"],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let results = search_palette(&pool, "/lib", "academia").await.unwrap();
        assert!(!results.tags.is_empty(), "should match tag substring");
        assert_eq!(results.tags[0].name, "Dark academia");
        assert_eq!(results.tags[0].book_count, 1);
    }

    /// Bug #1 (display side): the palette must show the overridden title,
    /// not the canonical scanned `b.title`, so what the user clicks matches
    /// what they searched for.
    #[tokio::test]
    async fn palette_book_hit_uses_overridden_title() {
        let _covers = CoversTempDir::new("palette_override_title");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "p.epub",
                Some("Scanned Title"),
                &["Author"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();
        let uuid = list_books(&pool, "/lib").await.unwrap()[0]
            .unique_identifier
            .clone()
            .unwrap();
        upsert_metadata_overrides(
            &pool,
            &uuid,
            &MetadataOverrides {
                title: Some("Edited Title".into()),
                ..Default::default()
            },
            false,
            user_id,
        )
        .await
        .unwrap();

        let palette = search_palette(&pool, "/lib", "Edited").await.unwrap();
        assert_eq!(palette.books.len(), 1);
        assert_eq!(palette.books[0].title, "Edited Title");
    }

    /// Bug #1 (display side): overriding the creators list rebuilds the
    /// comma-joined `author_display` so the palette subtitle matches the
    /// detail page.
    #[tokio::test]
    async fn palette_book_hit_uses_overridden_author_display() {
        let _covers = CoversTempDir::new("palette_override_authors");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "p.epub",
                Some("Searchable"),
                &["Original Author"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();
        let uuid = list_books(&pool, "/lib").await.unwrap()[0]
            .unique_identifier
            .clone()
            .unwrap();
        upsert_metadata_overrides(
            &pool,
            &uuid,
            &MetadataOverrides {
                creators: Some(vec![
                    Contributor {
                        name: "First Override".into(),
                        ..Default::default()
                    },
                    Contributor {
                        name: "Second Override".into(),
                        ..Default::default()
                    },
                ]),
                ..Default::default()
            },
            false,
            user_id,
        )
        .await
        .unwrap();

        let palette = search_palette(&pool, "/lib", "Searchable").await.unwrap();
        assert_eq!(palette.books.len(), 1);
        assert_eq!(
            palette.books[0].author_display,
            "First Override, Second Override"
        );
    }

    /// Palette book hits should surface a user-uploaded cover even when the
    /// scanned book had no cover. Mirrors `apply_overrides` so the palette
    /// row doesn't go cover-less for an override-only cover.
    #[tokio::test]
    async fn palette_book_hit_uses_overridden_cover() {
        let _covers = CoversTempDir::new("palette_override_cover");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        // Indexed book with no scanned cover.
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "p.epub",
                Some("Coverless Searchable"),
                &["Author"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();
        let book = list_books(&pool, "/lib").await.unwrap().remove(0);
        let uuid = book.unique_identifier.clone().unwrap();

        // Set has_cover_override = true with no text edits.
        upsert_metadata_overrides(&pool, &uuid, &MetadataOverrides::default(), true, user_id)
            .await
            .unwrap();

        let palette = search_palette(&pool, "/lib", "Coverless").await.unwrap();
        assert_eq!(palette.books.len(), 1);
        assert_eq!(
            palette.books[0].cover_url,
            Some(format!("/api/covers/{uuid}"))
        );
    }

    // #128: lock the wiring between the palette and `build_fts_match`'s
    // facet prefixes. A regression in the facet parser could otherwise
    // silently break palette tag:/author:/series: queries without any
    // palette test failing.

    #[tokio::test]
    async fn palette_book_matches_tag_facet() {
        let _covers = CoversTempDir::new("palette_tag_facet");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed(
                    "a.epub",
                    Some("Dracula"),
                    &["Bram Stoker"],
                    &["vampires"],
                    None,
                    None,
                ),
                indexed(
                    "b.epub",
                    Some("Frankenstein"),
                    &["Mary Shelley"],
                    &["monsters"],
                    None,
                    None,
                ),
            ],
        )
        .await
        .unwrap();

        let results = search_palette(&pool, "/lib", "tag:vampires").await.unwrap();
        let titles: Vec<&str> = results.books.iter().map(|b| b.title.as_str()).collect();
        assert!(
            titles.contains(&"Dracula"),
            "tag:vampires should match Dracula, got {titles:?}"
        );
        assert!(
            !titles.contains(&"Frankenstein"),
            "tag:vampires should not match Frankenstein"
        );
    }

    #[tokio::test]
    async fn palette_book_matches_author_facet() {
        let _covers = CoversTempDir::new("palette_author_facet");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed(
                    "a.epub",
                    Some("Dracula"),
                    &["Bram Stoker"],
                    &["horror"],
                    None,
                    None,
                ),
                indexed(
                    "b.epub",
                    Some("Frankenstein"),
                    &["Mary Shelley"],
                    &["horror"],
                    None,
                    None,
                ),
            ],
        )
        .await
        .unwrap();

        let results = search_palette(&pool, "/lib", "author:stoker")
            .await
            .unwrap();
        let titles: Vec<&str> = results.books.iter().map(|b| b.title.as_str()).collect();
        assert!(
            titles.contains(&"Dracula"),
            "author:stoker should match Dracula, got {titles:?}"
        );
        assert!(
            !titles.contains(&"Frankenstein"),
            "author:stoker should not match Frankenstein"
        );
    }

    #[tokio::test]
    async fn palette_book_matches_series_facet() {
        let _covers = CoversTempDir::new("palette_series_facet");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed(
                    "a.epub",
                    Some("Book One"),
                    &["Author A"],
                    &[],
                    Some(("Dracula Chronicles", "1")),
                    None,
                ),
                indexed("b.epub", Some("Unrelated"), &["Author B"], &[], None, None),
            ],
        )
        .await
        .unwrap();

        let results = search_palette(&pool, "/lib", "series:dracula")
            .await
            .unwrap();
        let titles: Vec<&str> = results.books.iter().map(|b| b.title.as_str()).collect();
        assert!(
            titles.contains(&"Book One"),
            "series:dracula should match Book One, got {titles:?}"
        );
        assert!(
            !titles.contains(&"Unrelated"),
            "series:dracula should not match Unrelated"
        );
    }

    #[tokio::test]
    async fn palette_empty_query_returns_empty() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let results = search_palette(&pool, "/lib", "   ").await.unwrap();
        assert!(results.books.is_empty());
        assert!(results.authors.is_empty());
        assert!(results.series.is_empty());
        assert!(results.tags.is_empty());
        assert_eq!(results.query, "");
    }

    #[tokio::test]
    async fn palette_no_results() {
        let _covers = CoversTempDir::new("palette_no_results");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed("a.epub", Some("A"), &["Author"], &[], None, None)],
        )
        .await
        .unwrap();

        let results = search_palette(&pool, "/lib", "zzzznonexistent")
            .await
            .unwrap();
        assert!(results.books.is_empty());
        assert!(results.authors.is_empty());
        assert!(results.series.is_empty());
        assert!(results.tags.is_empty());
    }

    #[tokio::test]
    async fn palette_scoped_to_library() {
        let _covers = CoversTempDir::new("palette_scoped");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib-a",
            vec![indexed(
                "a.epub",
                Some("Alpha"),
                &["Tolkien"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();
        replace_books(
            &pool,
            "/lib-b",
            vec![indexed(
                "b.epub",
                Some("Beta"),
                &["Tolkien"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let results = search_palette(&pool, "/lib-a", "tolkien").await.unwrap();
        // Books should only include lib-a
        assert_eq!(results.books.len(), 1);
        assert_eq!(results.books[0].title, "Alpha");
        // Author book_count should be 1 (scoped to lib-a), not 2
        assert_eq!(results.authors[0].book_count, 1);
    }

    #[tokio::test]
    async fn palette_duration_populated() {
        let _covers = CoversTempDir::new("palette_duration");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed("a.epub", Some("A"), &["Author"], &[], None, None)],
        )
        .await
        .unwrap();

        let results = search_palette(&pool, "/lib", "author").await.unwrap();
        // duration_ms should be populated (at least 0 — we just check it's set)
        assert!(results.duration_ms < 10000, "duration should be reasonable");
    }

    /// #127 regression coverage: after collapsing the correlated
    /// `book_count` / `EXISTS` subqueries into a single JOIN+GROUP BY,
    /// `l.path = ?1` must still be applied **before** the aggregate so the
    /// scoped library's count doesn't pick up rows from sibling libraries.
    /// This exercises all three taxonomies (authors, series, tags) plus
    /// ordering — the seeded set has 3 matching books in /lib-a and 2 in
    /// /lib-b for the same author/series/tag, and the rare "Sole" author
    /// only appears in /lib-b, so it must be absent from /lib-a results.
    #[tokio::test]
    async fn palette_taxonomy_counts_scoped_per_library() {
        let _covers = CoversTempDir::new("palette_taxonomy_scoped");
        let pool = init_db("sqlite::memory:").await.unwrap();

        replace_books(
            &pool,
            "/lib-a",
            vec![
                indexed(
                    "a1.epub",
                    Some("Alpha One"),
                    &["Shared Author"],
                    &["Shared Tag"],
                    Some(("Shared Series", "1")),
                    None,
                ),
                indexed(
                    "a2.epub",
                    Some("Alpha Two"),
                    &["Shared Author"],
                    &["Shared Tag"],
                    Some(("Shared Series", "2")),
                    None,
                ),
                indexed(
                    "a3.epub",
                    Some("Alpha Three"),
                    &["Shared Author"],
                    &["Shared Tag"],
                    Some(("Shared Series", "3")),
                    None,
                ),
            ],
        )
        .await
        .unwrap();
        replace_books(
            &pool,
            "/lib-b",
            vec![
                indexed(
                    "b1.epub",
                    Some("Beta One"),
                    &["Shared Author", "Sole Author"],
                    &["Shared Tag"],
                    Some(("Shared Series", "1")),
                    None,
                ),
                indexed(
                    "b2.epub",
                    Some("Beta Two"),
                    &["Shared Author"],
                    &["Shared Tag"],
                    Some(("Shared Series", "2")),
                    None,
                ),
            ],
        )
        .await
        .unwrap();

        // Scope to /lib-a — author/series/tag counts must be 3, not 5.
        let results = search_palette(&pool, "/lib-a", "Shared").await.unwrap();

        let author = results
            .authors
            .iter()
            .find(|a| a.name == "Shared Author")
            .expect("Shared Author present in /lib-a results");
        assert_eq!(
            author.book_count, 3,
            "author count must be scoped to /lib-a, got {results:?}"
        );
        assert!(
            !results.authors.iter().any(|a| a.name == "Sole Author"),
            "Sole Author lives only in /lib-b and must not appear"
        );

        let series = results
            .series
            .iter()
            .find(|s| s.name == "Shared Series")
            .expect("Shared Series present in /lib-a results");
        assert_eq!(
            series.book_count, 3,
            "series count must be scoped to /lib-a"
        );

        let tag = results
            .tags
            .iter()
            .find(|t| t.name == "Shared Tag")
            .expect("Shared Tag present in /lib-a results");
        assert_eq!(tag.book_count, 3, "tag count must be scoped to /lib-a");

        // Cross-check /lib-b counts to make sure the same query returns 2.
        let results_b = search_palette(&pool, "/lib-b", "Shared").await.unwrap();
        let author_b = results_b
            .authors
            .iter()
            .find(|a| a.name == "Shared Author")
            .expect("Shared Author present in /lib-b results");
        assert_eq!(author_b.book_count, 2);
        let series_b = results_b
            .series
            .iter()
            .find(|s| s.name == "Shared Series")
            .expect("Shared Series present in /lib-b results");
        assert_eq!(series_b.book_count, 2);
        let tag_b = results_b
            .tags
            .iter()
            .find(|t| t.name == "Shared Tag")
            .expect("Shared Tag present in /lib-b results");
        assert_eq!(tag_b.book_count, 2);
    }

    #[tokio::test]
    async fn palette_author_count_reflects_overrides() {
        // F5.1: the palette author count must match the merged
        // (override-aware) view, not the raw `books_authors_link` count.
        // Repro of the "Sanderson, Brandon still says 4 books" report:
        // every canonical book for an author was reassigned to a
        // differently-named author through the metadata edit form, so the
        // palette must report 0 books for the source name and the full
        // count for the destination name.
        let _covers = CoversTempDir::new("palette_author_count_overrides");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        // Two books canonically by "Last, First", plus one book by the
        // already-correct "First Last" so the destination author has a
        // canonical anchor (palette visibility requires ≥1 canonical link).
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed("a.epub", Some("A"), &["Last, First"], &[], None, None),
                indexed("b.epub", Some("B"), &["Last, First"], &[], None, None),
                indexed("c.epub", Some("C"), &["First Last"], &[], None, None),
            ],
        )
        .await
        .unwrap();

        // User edits a.epub and b.epub through the metadata form to
        // rename their author to "First Last" — overrides only, no
        // change to the relational link table.
        let books = list_books(&pool, "/lib").await.unwrap();
        for filename in ["a.epub", "b.epub"] {
            let book = books.iter().find(|b| b.filename == filename).unwrap();
            let uuid = book.unique_identifier.clone().unwrap();
            let ov = MetadataOverrides {
                creators: Some(vec![Contributor {
                    name: "First Last".into(),
                    role: Some("aut".into()),
                    file_as: None,
                    id: None,
                }]),
                ..Default::default()
            };
            upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
                .await
                .unwrap();
        }

        let results = search_palette(&pool, "/lib", "Last").await.unwrap();

        // Source author still visible (canonical anchor remains), but
        // count must reflect the effective view: 0 books.
        let source = results
            .authors
            .iter()
            .find(|a| a.name == "Last, First")
            .expect("source author still appears in palette");
        assert_eq!(
            source.book_count, 0,
            "renamed-away author must report effective count 0, got {results:?}",
        );

        // Destination author picks up the override-renamed books on top
        // of its own canonical anchor: 1 + 2 = 3.
        let dest = results
            .authors
            .iter()
            .find(|a| a.name == "First Last")
            .expect("destination author present");
        assert_eq!(
            dest.book_count, 3,
            "destination author must include override-renamed books, got {results:?}",
        );
    }

    #[tokio::test]
    async fn palette_tag_count_reflects_overrides() {
        // F5.1: same shape for tags. `overrides.subjects` replaces the
        // canonical tag list wholesale, so a book moved between tags
        // must shift both counts.
        let _covers = CoversTempDir::new("palette_tag_count_overrides");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        replace_books(
            &pool,
            "/lib",
            vec![
                indexed("a.epub", Some("A"), &["X"], &["tag-source"], None, None),
                indexed("b.epub", Some("B"), &["X"], &["tag-source"], None, None),
                indexed("c.epub", Some("C"), &["X"], &["tag-dest"], None, None),
            ],
        )
        .await
        .unwrap();

        // Move a.epub off tag-source and onto tag-dest via override.
        let books = list_books(&pool, "/lib").await.unwrap();
        let a = books.iter().find(|b| b.filename == "a.epub").unwrap();
        let uuid = a.unique_identifier.clone().unwrap();
        let ov = MetadataOverrides {
            subjects: Some(vec!["tag-dest".into()]),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let results = search_palette(&pool, "/lib", "tag-").await.unwrap();
        let source = results
            .tags
            .iter()
            .find(|t| t.name == "tag-source")
            .expect("tag-source still visible (canonical anchor remains)");
        assert_eq!(
            source.book_count, 1,
            "tag-source should drop a.epub after override, got {results:?}",
        );
        let dest = results
            .tags
            .iter()
            .find(|t| t.name == "tag-dest")
            .expect("tag-dest present");
        assert_eq!(
            dest.book_count, 2,
            "tag-dest should add the override-tagged a.epub, got {results:?}",
        );
    }

    #[tokio::test]
    async fn palette_series_count_reflects_overrides() {
        // F5.1: same shape as palette_author_count_reflects_overrides
        // but for the series tile. Books moved into a series via
        // `overrides.series` must add to the destination count; books
        // moved out drop from the source count.
        let _covers = CoversTempDir::new("palette_series_count_overrides");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        replace_books(
            &pool,
            "/lib",
            vec![
                indexed(
                    "a.epub",
                    Some("A"),
                    &["X"],
                    &[],
                    Some(("Series Source", "1")),
                    None,
                ),
                indexed(
                    "b.epub",
                    Some("B"),
                    &["X"],
                    &[],
                    Some(("Series Source", "2")),
                    None,
                ),
                indexed(
                    "c.epub",
                    Some("C"),
                    &["X"],
                    &[],
                    Some(("Series Dest", "1")),
                    None,
                ),
            ],
        )
        .await
        .unwrap();

        // Move a.epub from Series Source to Series Dest via override.
        let books = list_books(&pool, "/lib").await.unwrap();
        let a = books.iter().find(|b| b.filename == "a.epub").unwrap();
        let uuid = a.unique_identifier.clone().unwrap();
        let ov = MetadataOverrides {
            series: Some("Series Dest".into()),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        // "Series" matches both names.
        let results = search_palette(&pool, "/lib", "Series").await.unwrap();
        let source = results
            .series
            .iter()
            .find(|s| s.name == "Series Source")
            .expect("Series Source still visible (canonical anchor remains)");
        assert_eq!(
            source.book_count, 1,
            "Series Source should count only b.epub after a.epub is overridden away, got {results:?}",
        );
        let dest = results
            .series
            .iter()
            .find(|s| s.name == "Series Dest")
            .expect("Series Dest present");
        assert_eq!(
            dest.book_count, 2,
            "Series Dest should count its canonical c.epub plus the override-moved a.epub, got {results:?}",
        );
    }

    #[tokio::test]
    async fn palette_series_author_display_reflects_override() {
        // F5.1: the "by X" line on a series tile must follow the first
        // book's effective creator, not the canonical one — otherwise
        // renaming the author through the metadata edit form leaves the
        // palette showing the old name.
        let _covers = CoversTempDir::new("palette_series_author_display");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "k1.epub",
                Some("K1"),
                &["Old Name"],
                &[],
                Some(("Kingsway", "1")),
                None,
            )],
        )
        .await
        .unwrap();

        let books = list_books(&pool, "/lib").await.unwrap();
        let uuid = books[0].unique_identifier.clone().unwrap();

        let ov = MetadataOverrides {
            creators: Some(vec![Contributor {
                name: "New Name".into(),
                role: Some("aut".into()),
                file_as: None,
                id: None,
            }]),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let results = search_palette(&pool, "/lib", "Kingsway").await.unwrap();
        let kingsway = results
            .series
            .iter()
            .find(|s| s.name == "Kingsway")
            .expect("Kingsway present");
        assert_eq!(
            kingsway.author_display.as_deref(),
            Some("New Name"),
            "palette author line must follow override.creators, got {results:?}",
        );
    }

    /// #127: capture `EXPLAIN QUERY PLAN` for each of the three rewritten
    /// taxonomy queries and assert the planner uses the link-table indexes.
    /// This is a structural check — it doesn't pin the literal plan string
    /// (SQLite's wording can shift across point releases) but it does fail
    /// loudly if any of the link tables fall back to a full SCAN, which
    /// would defeat the whole point of this optimization.
    #[tokio::test]
    async fn palette_taxonomy_query_plans_use_indexes() {
        let pool = init_db("sqlite::memory:").await.unwrap();

        async fn plan_text(pool: &SqlitePool, sql: &str) -> String {
            let rows = sqlx::query(&format!("EXPLAIN QUERY PLAN {sql}"))
                .bind("/lib")
                .bind("%x%")
                .bind(5_i32)
                .fetch_all(pool)
                .await
                .unwrap();
            rows.iter()
                .map(|r| r.get::<String, _>("detail"))
                .collect::<Vec<_>>()
                .join("\n")
        }

        // Authors — override-aware count, must still drive through the
        // covering `books_authors_link` index when checking visibility
        // and when the no-override branch of the CASE runs.
        let plan = plan_text(
            &pool,
            "SELECT a.id, a.name, \
              (SELECT COUNT(*) FROM books b \
                 JOIN libraries l2 ON l2.id = b.library_id \
                 LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid \
                WHERE l2.path = ?1 \
                  AND CASE \
                        WHEN mo.book_uuid IS NOT NULL \
                             AND json_type(mo.overrides, '$.creators') IS NOT NULL \
                          THEN EXISTS (SELECT 1 FROM json_each(mo.overrides, '$.creators') je \
                                        WHERE json_extract(je.value, '$.name') = a.name) \
                        ELSE EXISTS (SELECT 1 FROM books_authors_link bal \
                                      WHERE bal.book = b.id AND bal.author = a.id) \
                      END \
              ) AS book_count \
             FROM authors a \
             WHERE a.name LIKE ?2 ESCAPE '\\' \
               AND EXISTS (SELECT 1 FROM books_authors_link bal \
                             JOIN books b ON b.id = bal.book \
                             JOIN libraries l ON l.id = b.library_id \
                            WHERE bal.author = a.id AND l.path = ?1) \
             ORDER BY book_count DESC, a.name \
             LIMIT ?3",
        )
        .await;
        assert!(
            !plan.contains("SCAN books_authors_link") && !plan.contains("SCAN bal"),
            "authors plan should not full-scan the link table:\n{plan}"
        );

        // Series — override-aware count + author_display, must still drive
        // through the `books_series_link` index for visibility and the
        // no-override branch of the CASE.
        let plan = plan_text(
            &pool,
            "SELECT s.id, s.name, \
              (SELECT COUNT(*) FROM books b \
                 JOIN libraries l2 ON l2.id = b.library_id \
                 LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid \
                WHERE l2.path = ?1 \
                  AND CASE \
                        WHEN mo.book_uuid IS NOT NULL \
                             AND json_type(mo.overrides, '$.series') IS NOT NULL \
                          THEN json_extract(mo.overrides, '$.series') = s.name \
                        ELSE EXISTS (SELECT 1 FROM books_series_link bsl \
                                      WHERE bsl.book = b.id AND bsl.series = s.id) \
                      END \
              ) AS book_count \
             FROM series s \
             WHERE s.name LIKE ?2 ESCAPE '\\' \
               AND EXISTS (SELECT 1 FROM books_series_link bsl \
                             JOIN books b ON b.id = bsl.book \
                             JOIN libraries l ON l.id = b.library_id \
                            WHERE bsl.series = s.id AND l.path = ?1) \
             ORDER BY book_count DESC, s.name \
             LIMIT ?3",
        )
        .await;
        assert!(
            !plan.contains("SCAN books_series_link") && !plan.contains("SCAN bsl"),
            "series plan should not full-scan the link table:\n{plan}"
        );

        // Tags — override-aware count, must still drive through the
        // `books_tags_link` index for visibility and the no-override
        // branch of the CASE.
        let plan = plan_text(
            &pool,
            "SELECT t.id, t.name, \
              (SELECT COUNT(*) FROM books b \
                 JOIN libraries l2 ON l2.id = b.library_id \
                 LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid \
                WHERE l2.path = ?1 \
                  AND CASE \
                        WHEN mo.book_uuid IS NOT NULL \
                             AND json_type(mo.overrides, '$.subjects') IS NOT NULL \
                          THEN EXISTS (SELECT 1 FROM json_each(mo.overrides, '$.subjects') je \
                                        WHERE je.value = t.name) \
                        ELSE EXISTS (SELECT 1 FROM books_tags_link btl \
                                      WHERE btl.book = b.id AND btl.tag = t.id) \
                      END \
              ) AS book_count \
             FROM tags t \
             WHERE t.name LIKE ?2 ESCAPE '\\' \
               AND EXISTS (SELECT 1 FROM books_tags_link btl \
                             JOIN books b ON b.id = btl.book \
                             JOIN libraries l ON l.id = b.library_id \
                            WHERE btl.tag = t.id AND l.path = ?1) \
             ORDER BY book_count DESC, t.name \
             LIMIT ?3",
        )
        .await;
        assert!(
            !plan.contains("SCAN books_tags_link") && !plan.contains("SCAN btl"),
            "tags plan should not full-scan the link table:\n{plan}"
        );
    }

    // Additional coverage for core book query functions.

    #[tokio::test]
    async fn list_books_filters_by_library_path() {
        let _covers = CoversTempDir::new("list_books_filter_lib");
        let pool = init_db("sqlite::memory:").await.unwrap();

        replace_books(
            &pool,
            "/lib-a",
            vec![
                indexed("a1.epub", Some("Alpha One"), &["Author A"], &[], None, None),
                indexed("a2.epub", Some("Alpha Two"), &["Author A"], &[], None, None),
            ],
        )
        .await
        .unwrap();
        replace_books(
            &pool,
            "/lib-b",
            vec![indexed(
                "b1.epub",
                Some("Beta One"),
                &["Author B"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let lib_a = list_books(&pool, "/lib-a").await.unwrap();
        let lib_b = list_books(&pool, "/lib-b").await.unwrap();

        assert_eq!(lib_a.len(), 2, "lib-a should return only its two books");
        let mut titles_a: Vec<String> = lib_a.iter().filter_map(|b| b.title.clone()).collect();
        titles_a.sort();
        assert_eq!(titles_a, vec!["Alpha One", "Alpha Two"]);

        assert_eq!(lib_b.len(), 1, "lib-b should return only its one book");
        assert_eq!(lib_b[0].title.as_deref(), Some("Beta One"));
    }

    #[tokio::test]
    async fn list_books_returns_empty_for_unknown_path() {
        let _covers = CoversTempDir::new("list_books_unknown");
        let pool = init_db("sqlite::memory:").await.unwrap();

        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "a.epub",
                Some("Title"),
                &["Author"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let hits = list_books(&pool, "/does-not-exist").await.unwrap();
        assert!(
            hits.is_empty(),
            "unknown library path should yield an empty vec (no error)"
        );
    }

    #[tokio::test]
    async fn list_books_returns_empty_for_empty_db() {
        let _covers = CoversTempDir::new("list_books_empty_db");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let hits = list_books(&pool, "/lib").await.unwrap();
        assert!(hits.is_empty(), "empty DB should yield an empty vec");
    }

    #[tokio::test]
    async fn search_books_handles_bare_asterisk_without_error() {
        // A raw `*` is an FTS5 operator; the sanitizer must quote it so MATCH
        // doesn't reject the expression as a syntax error. We assert the call
        // succeeds — not a particular hit shape — because the goal is "no panic
        // / no sqlx parse error".
        let _covers = CoversTempDir::new("fts_asterisk");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "a.epub",
                Some("Anything"),
                &["Author"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let hits = search_books(&pool, "/lib", "*")
            .await
            .expect("sanitizer should guard MATCH against the bare `*` operator");
        // `*` alone has no literal token to match; an empty result is fine.
        assert!(hits.is_empty());
    }

    #[tokio::test]
    async fn search_books_handles_bare_double_quote_without_error() {
        // A raw `"` is the FTS5 phrase delimiter. Without sanitization, MATCH
        // would reject this with a parse error and the call would `Err`.
        let _covers = CoversTempDir::new("fts_dquote");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "a.epub",
                Some("Anything"),
                &["Author"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let hits = search_books(&pool, "/lib", "\"")
            .await
            .expect("sanitizer should guard MATCH against a bare `\"` operator");
        assert!(hits.is_empty());
    }

    #[tokio::test]
    async fn search_books_returns_empty_for_unknown_library() {
        // Even with a real match in another library, the WHERE l.path = ?
        // clause must scope results to the requested library.
        let _covers = CoversTempDir::new("search_books_unknown_lib");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib-a",
            vec![indexed(
                "a.epub",
                Some("Findable"),
                &["Author"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let hits = search_books(&pool, "/lib-b", "Findable").await.unwrap();
        assert!(
            hits.is_empty(),
            "query against a non-existent library must not leak rows from another library"
        );
    }

    // -------------------------------------------------------------------------
    // F1.11 author profile photo tests
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn author_photo_roundtrips_manual_upload() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let ada_id = author_id_by_name(&pool, "Ada Lovelace").await;

        let bytes = b"\xFF\xD8\xFFfake-jpeg".to_vec();
        upsert_author_photo(
            &pool,
            ada_id,
            AuthorPhotoSource::Manual,
            None,
            Some("image/jpeg"),
            Some(&bytes),
        )
        .await
        .unwrap();

        let (mime, fetched) = get_author_photo(&pool, ada_id).await.unwrap().unwrap();
        assert_eq!(mime, "image/jpeg");
        assert_eq!(fetched, bytes);
    }

    #[tokio::test]
    async fn author_photo_letter_marker_returns_none() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let ada_id = author_id_by_name(&pool, "Ada Lovelace").await;

        upsert_author_photo(&pool, ada_id, AuthorPhotoSource::Letter, None, None, None)
            .await
            .unwrap();

        assert!(get_author_photo(&pool, ada_id).await.unwrap().is_none());

        let (src, _) = author_photo_status(&pool, ada_id).await.unwrap().unwrap();
        assert_eq!(src, AuthorPhotoSource::Letter);
    }

    #[tokio::test]
    async fn author_photo_status_none_when_unset() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let ada_id = author_id_by_name(&pool, "Ada Lovelace").await;
        assert!(author_photo_status(&pool, ada_id).await.unwrap().is_none());
        assert!(get_author_photo(&pool, ada_id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn author_photo_upsert_replaces_existing_row() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let ada_id = author_id_by_name(&pool, "Ada Lovelace").await;

        // Letter marker first, then a manual upload replaces it.
        upsert_author_photo(&pool, ada_id, AuthorPhotoSource::Letter, None, None, None)
            .await
            .unwrap();
        upsert_author_photo(
            &pool,
            ada_id,
            AuthorPhotoSource::Manual,
            None,
            Some("image/png"),
            Some(b"\x89PNG\r\n\x1a\nfake"),
        )
        .await
        .unwrap();

        let (src, _) = author_photo_status(&pool, ada_id).await.unwrap().unwrap();
        assert_eq!(src, AuthorPhotoSource::Manual);
        let (mime, _) = get_author_photo(&pool, ada_id).await.unwrap().unwrap();
        assert_eq!(mime, "image/png");
    }

    #[tokio::test]
    async fn author_photo_delete_clears_row() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let ada_id = author_id_by_name(&pool, "Ada Lovelace").await;

        upsert_author_photo(
            &pool,
            ada_id,
            AuthorPhotoSource::Manual,
            None,
            Some("image/jpeg"),
            Some(b"\xFF\xD8\xFFfoo"),
        )
        .await
        .unwrap();
        delete_author_photo(&pool, ada_id).await.unwrap();

        assert!(author_photo_status(&pool, ada_id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn get_author_populates_has_photo() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let ada_id = author_id_by_name(&pool, "Ada Lovelace").await;

        // No row → false.
        let ada = get_author(&pool, ada_id).await.unwrap().unwrap();
        assert!(!ada.has_photo, "no row should yield has_photo = false");

        // Letter marker → still false (negative-cache shouldn't render an img).
        upsert_author_photo(&pool, ada_id, AuthorPhotoSource::Letter, None, None, None)
            .await
            .unwrap();
        let ada = get_author(&pool, ada_id).await.unwrap().unwrap();
        assert!(
            !ada.has_photo,
            "letter marker should yield has_photo = false"
        );

        // Manual upload → true.
        upsert_author_photo(
            &pool,
            ada_id,
            AuthorPhotoSource::Manual,
            None,
            Some("image/jpeg"),
            Some(b"\xFF\xD8\xFFfake"),
        )
        .await
        .unwrap();
        let ada = get_author(&pool, ada_id).await.unwrap().unwrap();
        assert!(ada.has_photo, "manual upload should yield has_photo = true");
    }

    /// `list_authors` mirrors the `get_author` has_photo semantics so the
    /// /authors index can pick the right avatar without a per-card detail
    /// fetch. Same three-state matrix: no row → false, `letter` marker →
    /// false, `manual` upload → true.
    #[tokio::test]
    async fn list_authors_populates_has_photo() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let ada_id = author_id_by_name(&pool, "Ada Lovelace").await;

        let initial = list_authors(&pool, "/lib").await.unwrap();
        let ada = initial.iter().find(|a| a.id == ada_id).unwrap();
        assert!(!ada.has_photo, "no row should yield has_photo = false");

        upsert_author_photo(&pool, ada_id, AuthorPhotoSource::Letter, None, None, None)
            .await
            .unwrap();
        let after_letter = list_authors(&pool, "/lib").await.unwrap();
        let ada = after_letter.iter().find(|a| a.id == ada_id).unwrap();
        assert!(
            !ada.has_photo,
            "letter marker should yield has_photo = false"
        );

        upsert_author_photo(
            &pool,
            ada_id,
            AuthorPhotoSource::Manual,
            None,
            Some("image/jpeg"),
            Some(b"\xFF\xD8\xFFfake"),
        )
        .await
        .unwrap();
        let after_upload = list_authors(&pool, "/lib").await.unwrap();
        let ada = after_upload.iter().find(|a| a.id == ada_id).unwrap();
        assert!(ada.has_photo, "manual upload should yield has_photo = true");
    }

    // -------------------------------------------------------------------------
    // ignored_authors blocklist (F5.9-lite reindex-resurrection guard)
    // -------------------------------------------------------------------------

    // -------------------------------------------------------------------------
    // delete_author (F5.9-lite admin "Delete author" primitive)
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn delete_author_removes_links_and_inserts_blocklist_row() {
        let _covers = CoversTempDir::new("delete_author_basic");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed("a.epub", Some("A"), &["Junk Author"], &[], None, None),
                indexed(
                    "b.epub",
                    Some("B"),
                    &["Junk Author", "Real Author"],
                    &[],
                    None,
                    None,
                ),
            ],
        )
        .await
        .unwrap();

        let junk_id: i64 = sqlx::query_scalar("SELECT id FROM authors WHERE name = ?")
            .bind("Junk Author")
            .fetch_one(&pool)
            .await
            .unwrap();

        let unlinked = delete_author(&pool, junk_id).await.unwrap();
        assert_eq!(unlinked, 2, "both books should report as un-linked");

        let junk_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM authors WHERE id = ?")
            .bind(junk_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(junk_count, 0, "authors row should be gone");

        let link_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM books_authors_link WHERE author = ?")
                .bind(junk_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            link_count, 0,
            "no link rows should remain for deleted author"
        );

        let blocklist_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM ignored_authors WHERE name = ?")
                .bind("Junk Author")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(blocklist_count, 1, "name must be added to ignored_authors");

        // Real Author on book B should still be linked.
        let books = list_books(&pool, "/lib").await.unwrap();
        let b = books
            .iter()
            .find(|x| x.title.as_deref() == Some("B"))
            .unwrap();
        let creators: Vec<&str> = b.creators.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(creators, vec!["Real Author"]);
    }

    #[tokio::test]
    async fn delete_author_is_no_op_for_missing_id() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let unlinked = delete_author(&pool, 99_999).await.unwrap();
        assert_eq!(unlinked, 0);
        let blocklist_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ignored_authors")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            blocklist_count, 0,
            "missing id must not leak a ghost blocklist row"
        );
    }

    #[tokio::test]
    async fn delete_author_survives_reindex() {
        // Full durability check: delete the junk author, then run the
        // reindex pipeline against a fixture that *still* lists the junk
        // contributor in its OPF (simulated via replace_books with the
        // same input). The blocklist row inserted by delete_author must
        // keep resolve_or_insert_author from re-creating the author.
        let _covers = CoversTempDir::new("delete_author_reindex");
        let pool = init_db("sqlite::memory:").await.unwrap();

        let junk_name = "calibre (8.0.0) [https://calibre-ebook.com]";
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "a.epub",
                Some("A"),
                &["Real Author", junk_name],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let junk_id: i64 = sqlx::query_scalar("SELECT id FROM authors WHERE name = ?")
            .bind(junk_name)
            .fetch_one(&pool)
            .await
            .unwrap();
        delete_author(&pool, junk_id).await.unwrap();

        // Simulated reindex: same OPF contents, run through the
        // indexing pipeline again. Without the blocklist guard this
        // would re-create the junk author and relink it.
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "a.epub",
                Some("A"),
                &["Real Author", junk_name],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let junk_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM authors WHERE name = ?")
            .bind(junk_name)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            junk_after, 0,
            "second reindex must not resurrect a deleted author"
        );

        let books = list_books(&pool, "/lib").await.unwrap();
        let creators: Vec<&str> = books[0].creators.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(creators, vec!["Real Author"]);
    }

    #[tokio::test]
    async fn reindex_path_skips_blocked_contributor_and_keeps_real_author() {
        // End-to-end: simulate the reindex path by going through
        // `replace_books` (same insert_metadata_links pipeline). The
        // blocklist guard must keep the junk row from being re-created
        // while the legitimate author is still linked to the book.
        let _covers = CoversTempDir::new("ignored_authors_reindex");
        let pool = init_db("sqlite::memory:").await.unwrap();

        sqlx::query("INSERT INTO ignored_authors(name) VALUES (?)")
            .bind("calibre (8.0.0) [https://calibre-ebook.com]")
            .execute(&pool)
            .await
            .unwrap();

        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "the-real-book.epub",
                Some("The Real Book"),
                &["Real Author", "calibre (8.0.0) [https://calibre-ebook.com]"],
                &[],
                None,
                None,
            )],
        )
        .await
        .expect("reindex path should succeed even with a blocked contributor");

        let junk_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM authors WHERE name = ?")
            .bind("calibre (8.0.0) [https://calibre-ebook.com]")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(junk_count, 0, "junk author must not be re-created");

        let books = list_books(&pool, "/lib").await.unwrap();
        assert_eq!(books.len(), 1);
        let book = &books[0];
        let names: Vec<&str> = book.creators.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["Real Author"],
            "only the un-blocked creator should remain linked"
        );
    }
}

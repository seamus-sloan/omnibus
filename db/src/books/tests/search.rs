//! `search_books`: BM25 ranking, author and library scoping, the facet
//! filters, and the query-sanitization paths (oversized, empty, unbalanced
//! quote, bare asterisk).

use omnibus_shared::Identifier;

use crate::ebook::IndexedBook;
use crate::helpers::MAX_QUERY_LEN;
use crate::pool::init_db;
use crate::sync::replace_books;
use crate::test_support::{indexed, CoversTempDir};

use super::super::*;

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
async fn search_books_with_total_matches_count_search_books() {
    // #241: the single FTS5 pass must return the same true hit count the
    // standalone `count_search_books` query produced.
    let _covers = CoversTempDir::new("fts_total");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![
            indexed("a.epub", Some("Rust in Action"), &["A"], &[], None, None),
            indexed(
                "b.epub",
                Some("Rust for Rustaceans"),
                &["B"],
                &[],
                None,
                None,
            ),
            indexed("c.epub", Some("Unrelated"), &["C"], &[], None, None),
        ],
    )
    .await
    .unwrap();

    let (books, total) = search_books_with_total(&pool, "/lib", "rust")
        .await
        .unwrap();
    assert_eq!(books.len(), 2, "two titles match 'rust'");
    assert_eq!(
        total, 2,
        "single-pass total (scalar COUNT over the materialized CTE) equals the match count"
    );
    let counted = count_search_books(&pool, "/lib", "rust").await.unwrap();
    assert_eq!(
        total, counted,
        "single-pass total agrees with count_search_books"
    );

    // Empty query short-circuits to (empty, 0) without an FTS pass.
    let (empty, zero) = search_books_with_total(&pool, "/lib", "   ").await.unwrap();
    assert!(empty.is_empty());
    assert_eq!(zero, 0);
}

/// Seed a two-book library and put `genres` on the first through the
/// override write path — the only way a book gets genres (migration `0066`).
/// Returns the pool, the id of the user the overrides are attributed to, and
/// the covers guard the caller must keep alive.
async fn seed_genre_fixture(tag: &str, genres: &[&str]) -> (sqlx::SqlitePool, i64, CoversTempDir) {
    let covers = CoversTempDir::new(tag);
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![
            indexed("a.epub", Some("Dark Water"), &["Ann"], &[], None, None),
            indexed("b.epub", Some("Bright Sky"), &["Bob"], &[], None, None),
        ],
    )
    .await
    .unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    set_genres(&pool, "Dark Water", genres, user_id).await;
    (pool, user_id, covers)
}

/// Replace the genre list on the book titled `title` via the override door.
async fn set_genres(pool: &sqlx::SqlitePool, title: &str, genres: &[&str], user_id: i64) {
    let books = crate::books::list_books(pool, "/lib").await.unwrap();
    let uuid = books
        .iter()
        .find(|b| b.title.as_deref() == Some(title))
        .and_then(|b| b.unique_identifier.clone())
        .expect("seeded book");
    crate::metadata_overrides::merge_metadata_overrides(
        pool,
        &uuid,
        &omnibus_shared::MetadataOverrides {
            genres: Some(genres.iter().map(|g| (*g).to_string()).collect()),
            ..Default::default()
        },
        user_id,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn search_books_finds_by_genre_facet_right_after_an_override_save() {
    // Genres reach the index through the same post-write FTS refresh a title
    // or tag edit takes, so a genre is searchable with no reindex in between.
    let (pool, _user, _covers) = seed_genre_fixture("fts_genre", &["Horror", "Gothic"]).await;

    let hits = search_books(&pool, "/lib", "genre:horror").await.unwrap();
    assert_eq!(hits.len(), 1, "only the overridden book carries Horror");
    assert_eq!(hits[0].title.as_deref(), Some("Dark Water"));

    // Every genre in the list indexes, not just the first.
    let hits = search_books(&pool, "/lib", "genre:gothic").await.unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].title.as_deref(), Some("Dark Water"));

    // A genre no book carries matches nothing.
    assert!(search_books(&pool, "/lib", "genre:western")
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn search_books_genre_facet_drops_a_book_whose_genres_were_cleared() {
    // An empty list is the clear-all override, and it has to reach the index
    // — a stale `genres` column would keep serving a genre the book lost.
    let (pool, user_id, _covers) = seed_genre_fixture("fts_genre_clear", &["Horror"]).await;
    assert_eq!(
        search_books(&pool, "/lib", "genre:horror")
            .await
            .unwrap()
            .len(),
        1
    );

    set_genres(&pool, "Dark Water", &[], user_id).await;

    assert!(
        search_books(&pool, "/lib", "genre:horror")
            .await
            .unwrap()
            .is_empty(),
        "a cleared genre must leave genre:-scoped results"
    );
}

#[tokio::test]
async fn search_books_genre_facet_drops_a_book_whose_overrides_were_deleted() {
    // Deleting the overrides row removes the genres' only storage, and the
    // delete path restores canonical FTS in its own transaction — the
    // restored row must carry an empty `genres`, not the deleted list.
    let (pool, _user, _covers) = seed_genre_fixture("fts_genre_delete", &["Horror"]).await;
    let books = crate::books::list_books(&pool, "/lib").await.unwrap();
    let uuid = books
        .iter()
        .find(|b| b.title.as_deref() == Some("Dark Water"))
        .and_then(|b| b.unique_identifier.clone())
        .expect("seeded book");

    crate::metadata_overrides::delete_metadata_overrides(&pool, &uuid)
        .await
        .unwrap();

    assert!(
        search_books(&pool, "/lib", "genre:horror")
            .await
            .unwrap()
            .is_empty(),
        "a deleted override must leave genre:-scoped results"
    );
}

#[tokio::test]
async fn search_books_genre_facet_ignores_overrides_on_an_embedded_tags_first_root() {
    // `apply_overrides` returns before applying genres when the scan root
    // ranks embedded metadata above the override layer, so the book's
    // effective genres are empty. The FTS door must agree — otherwise
    // `genre:` answers for a book whose own detail page shows no genres, and
    // the door and `overlay_overrides` (which sources this column from the
    // precedence-gated merge) disagree inside a single transaction.
    let (pool, user_id, _covers) = seed_genre_fixture("fts_genre_precedence", &["Horror"]).await;
    assert_eq!(
        search_books(&pool, "/lib", "genre:horror")
            .await
            .unwrap()
            .len(),
        1,
        "default precedence lets the override through"
    );

    sqlx::query(
        r#"UPDATE scan_roots SET metadata_precedence =
             '["folder_structure","omnibus_overrides","opf_sidecar","embedded_tags","provider_match"]'
           WHERE path = '/lib'"#,
    )
    .execute(&pool)
    .await
    .unwrap();
    // Re-run the write door so the index reflects the new precedence.
    set_genres(&pool, "Dark Water", &["Horror"], user_id).await;

    assert!(
        search_books(&pool, "/lib", "genre:horror")
            .await
            .unwrap()
            .is_empty(),
        "embedded-tags-first must keep override genres out of the index"
    );
}

#[tokio::test]
async fn search_books_tolerates_a_corrupt_overrides_blob() {
    // `json_each` raises `malformed JSON` on a bad blob, and the genres
    // projection now runs on the write path — so one corrupt row would fail
    // every reindex and the admin rebuild, not just one read.
    let (pool, _user, _covers) = seed_genre_fixture("fts_genre_corrupt", &["Horror"]).await;
    let books = crate::books::list_books(&pool, "/lib").await.unwrap();
    let uuid = books
        .iter()
        .find(|b| b.title.as_deref() == Some("Bright Sky"))
        .and_then(|b| b.unique_identifier.clone())
        .expect("seeded book");
    sqlx::query("INSERT INTO metadata_overrides (book_uuid, overrides) VALUES (?, ?)")
        .bind(&uuid)
        .bind("{ not valid json")
        .execute(&pool)
        .await
        .unwrap();

    crate::sync::rebuild_all_fts(&pool)
        .await
        .expect("a corrupt blob must not fail the whole-index rebuild");

    // The healthy book is still indexed; the corrupt one simply has none.
    assert_eq!(
        search_books(&pool, "/lib", "genre:horror")
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn search_books_free_text_does_not_match_a_genre_name() {
    // Same reason `tags` sits outside the default scope: typing "Dra" must
    // not drag in every book someone genred "Drama".
    let (pool, _user, _covers) = seed_genre_fixture("fts_genre_scope", &["Drama"]).await;

    assert!(
        search_books(&pool, "/lib", "Dra").await.unwrap().is_empty(),
        "free text stays scoped to {{title authors series}}"
    );
    assert_eq!(
        search_books(&pool, "/lib", "genre:Dra")
            .await
            .unwrap()
            .len(),
        1,
        "the same prefix does hit once it is genre:-scoped"
    );
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
async fn search_books_truncates_oversized_query() {
    // Issue #189: a query longer than MAX_QUERY_LEN chars must be capped
    // before reaching build_fts_match, not panic or pass an unbounded
    // expression to FTS5. The exact rows don't matter — this documents
    // the contract that oversized input is bounded and returns Ok.
    let _covers = CoversTempDir::new("fts_oversized");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "a.epub",
            Some("Harry Potter"),
            &["J.K. Rowling"],
            &[],
            None,
            None,
        )],
    )
    .await
    .unwrap();

    // A single token far longer than the cap (no whitespace) is the
    // worst case the issue describes.
    let oversized = "a".repeat(MAX_QUERY_LEN * 10);
    assert!(oversized.chars().count() > MAX_QUERY_LEN);

    let hits = search_books(&pool, "/lib", &oversized).await;
    assert!(hits.is_ok(), "oversized query should not error");

    let total = count_search_books(&pool, "/lib", &oversized).await;
    assert!(total.is_ok(), "oversized count query should not error");
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
            word_count: None,
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
        "INSERT INTO book_files (book_id, format, filename, size_bytes)
         VALUES (?, 'M4B', 'alpha', 0)",
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

//! Pure replica pagination / sorting / search tests.
// The state-lock guard is deliberately held across awaits: it serializes
// whole async test bodies against process-global state, and each test owns
// its own thread + runtime, so there is no interleaving to deadlock on.
#![allow(clippy::await_holding_lock)]

use omnibus_shared::Contributor;

use super::*;

fn book(title: &str, author: &str, formats: &[&str]) -> EbookMetadata {
    EbookMetadata {
        title: Some(title.to_string()),
        filename: format!("{title}.epub"),
        creators: vec![Contributor {
            name: author.to_string(),
            ..Default::default()
        }],
        formats: formats.iter().map(|f| f.to_string()).collect(),
        unique_identifier: Some(format!("uuid-{title}")),
        ..Default::default()
    }
}

fn titles(page: &LibraryPage) -> Vec<String> {
    page.books
        .iter()
        .map(|b| b.title.clone().unwrap_or_default())
        .collect()
}

fn fixture() -> Vec<EbookMetadata> {
    vec![
        book("Cider House", "Irving", &["EPUB"]),
        book("Atonement", "McEwan", &["EPUB", "M4B"]),
        book("Beloved", "Morrison", &["M4B"]),
        book("Dune", "Herbert", &["EPUB"]),
    ]
}

#[test]
fn page_from_replica_sorts_by_title_and_slices_by_cursor() {
    let first = page_from_replica(fixture(), SortKey::Title, SortDir::Asc, &[], &[], None, 2);
    assert_eq!(titles(&first), vec!["Atonement", "Beloved"]);
    assert_eq!(first.total, Some(4));
    assert_eq!(first.next_cursor.as_deref(), Some("off:2"));

    let second = page_from_replica(
        fixture(),
        SortKey::Title,
        SortDir::Asc,
        &[],
        &[],
        Some("off:2"),
        2,
    );
    assert_eq!(titles(&second), vec!["Cider House", "Dune"]);
    // `total` is a first-page-only field, mirroring the online headers.
    assert_eq!(second.total, None);
    assert_eq!(second.next_cursor, None);
}

#[test]
fn page_from_replica_flips_direction() {
    let page = page_from_replica(fixture(), SortKey::Title, SortDir::Desc, &[], &[], None, 10);
    assert_eq!(
        titles(&page),
        vec!["Dune", "Cider House", "Beloved", "Atonement"]
    );
}

#[test]
fn page_from_replica_sorts_by_author_with_title_tiebreak() {
    let mut books = fixture();
    books.push(book("A Widow for One Year", "Irving", &["EPUB"]));
    let page = page_from_replica(books, SortKey::Author, SortDir::Asc, &[], &[], None, 10);
    assert_eq!(
        titles(&page),
        vec![
            "Dune",                 // Herbert
            "A Widow for One Year", // Irving (title tiebreak)
            "Cider House",          // Irving
            "Atonement",            // McEwan
            "Beloved",              // Morrison
        ]
    );
}

#[test]
fn page_from_replica_filters_by_format_any_match() {
    let page = page_from_replica(
        fixture(),
        SortKey::Title,
        SortDir::Asc,
        &["m4b".to_string()],
        &[],
        None,
        10,
    );
    assert_eq!(titles(&page), vec!["Atonement", "Beloved"]);
    assert_eq!(page.total, Some(2));
}

#[test]
fn page_from_replica_ends_stream_on_foreign_cursor() {
    // A leftover *online* keyset cursor must not restart pagination from the
    // top (duplicate grid keys); it ends the stream instead.
    let page = page_from_replica(
        fixture(),
        SortKey::Title,
        SortDir::Asc,
        &[],
        &[],
        Some("eyJvbmxpbmUiOiJjdXJzb3IifQ"),
        10,
    );
    assert!(page.books.is_empty());
    assert_eq!(page.next_cursor, None);
}

#[test]
fn page_from_replica_returns_empty_page_past_the_end() {
    let page = page_from_replica(
        fixture(),
        SortKey::Title,
        SortDir::Asc,
        &[],
        &[],
        Some("off:99"),
        10,
    );
    assert!(page.books.is_empty());
    assert_eq!(page.next_cursor, None);
}

fn book_with_series(
    title: &str,
    series: Option<&str>,
    series_index: Option<&str>,
) -> EbookMetadata {
    EbookMetadata {
        title: Some(title.to_string()),
        filename: format!("{title}.epub"),
        series: series.map(str::to_string),
        series_index: series_index.map(str::to_string),
        unique_identifier: Some(format!("uuid-{title}")),
        ..Default::default()
    }
}

fn book_with_dates(title: &str, added_at: Option<&str>, modified: Option<&str>) -> EbookMetadata {
    EbookMetadata {
        title: Some(title.to_string()),
        filename: format!("{title}.epub"),
        added_at: added_at.map(str::to_string),
        modified: modified.map(str::to_string),
        unique_identifier: Some(format!("uuid-{title}")),
        ..Default::default()
    }
}

#[test]
fn sort_books_orders_by_series_then_title_with_no_series_sorting_last() {
    let mut books = vec![
        book_with_series("Solo", None, None),
        book_with_series("Second In Series", Some("Wheel"), Some("2")),
        book_with_series("First In Series", Some("Wheel"), Some("1")),
        book_with_series("Another Series", Some("Annals"), Some("1")),
    ];
    sort_books(&mut books, SortKey::Series, SortDir::Asc);
    let titles: Vec<&str> = books.iter().map(|b| b.title.as_deref().unwrap()).collect();
    assert_eq!(
        titles,
        vec![
            "Another Series",   // Annals, index 1
            "First In Series",  // Wheel, index 1
            "Second In Series", // Wheel, index 2
            "Solo",             // no series — sorts last
        ]
    );
}

#[test]
fn sort_books_orders_by_last_updated_and_newest_added_on_the_same_date_key() {
    let mut books = vec![
        book_with_dates("No Date", None, None),
        book_with_dates("Older", Some("2024-01-01"), None),
        book_with_dates("Modified Only", None, Some("2024-06-01")),
    ];
    sort_books(&mut books, SortKey::LastUpdated, SortDir::Asc);
    let titles: Vec<&str> = books.iter().map(|b| b.title.as_deref().unwrap()).collect();
    // Empty date keys sort first; `NewestAdded` shares the exact same
    // comparator (see `sort_books`'s match arm), so it must agree too.
    assert_eq!(titles, vec!["No Date", "Older", "Modified Only"]);
    let mut same = vec![
        book_with_dates("No Date", None, None),
        book_with_dates("Older", Some("2024-01-01"), None),
        book_with_dates("Modified Only", None, Some("2024-06-01")),
    ];
    sort_books(&mut same, SortKey::NewestAdded, SortDir::Asc);
    let same_titles: Vec<&str> = same.iter().map(|b| b.title.as_deref().unwrap()).collect();
    assert_eq!(same_titles, titles);
}

#[test]
fn sort_books_orders_recently_interacted_on_its_own_key_not_the_date_fallback() {
    let interacted = |title: &str, at: Option<&str>, added_at: Option<&str>| EbookMetadata {
        title: Some(title.to_string()),
        filename: format!("{title}.epub"),
        added_at: added_at.map(str::to_string),
        last_interacted_at: at.map(str::to_string),
        unique_identifier: Some(format!("uuid-{title}")),
        ..Default::default()
    };
    // Fixed-width ISO, as the projection emits it: the replica compares these
    // lexicographically, which is only chronologically correct in that format.
    // `added_at` runs opposite to `last_interacted_at`, so a comparator that
    // fell back to the date key would produce the reverse of this order.
    let mut books = vec![
        interacted(
            "Rated Today",
            Some("2026-05-01T09:00:00Z"),
            Some("2020-01-01T00:00:00Z"),
        ),
        interacted("Untouched", None, Some("2026-01-01T00:00:00Z")),
        interacted(
            "Rated Last Year",
            Some("2025-05-01T09:00:00Z"),
            Some("2023-01-01T00:00:00Z"),
        ),
    ];
    sort_books(&mut books, SortKey::RecentlyInteracted, SortDir::Desc);
    let titles: Vec<&str> = books.iter().map(|b| b.title.as_deref().unwrap()).collect();
    assert_eq!(titles, vec!["Rated Today", "Rated Last Year", "Untouched"]);
}

#[test]
fn series_key_truncates_the_float_cast_toward_zero_at_the_millesimal_boundary() {
    let just_under_two = book_with_series("A", Some("S"), Some("1.9999"));
    let exactly_two = book_with_series("B", Some("S"), Some("2.0"));
    // `1.9999 * 1000.0 == 1999.9`, cast to i64 truncates to 1999 — not 2000 —
    // so it must still sort strictly before the exact "2.0" (2000) index.
    assert_eq!(series_key(&just_under_two), (false, "s".to_string(), 1999));
    assert_eq!(series_key(&exactly_two), (false, "s".to_string(), 2000));
}

#[test]
fn series_key_falls_back_to_zero_for_missing_or_unparsable_index() {
    let no_index = book_with_series("A", Some("S"), None);
    let bad_index = book_with_series("B", Some("S"), Some("not-a-number"));
    assert_eq!(series_key(&no_index).2, 0);
    assert_eq!(series_key(&bad_index).2, 0);
}

#[test]
fn series_key_marks_books_without_a_series_to_sort_after_every_series() {
    let with_series = book_with_series("A", Some("S"), Some("1"));
    let without_series = book_with_series("B", None, None);
    assert!(!series_key(&with_series).0);
    assert!(series_key(&without_series).0);
}

#[test]
fn date_key_prefers_added_at_then_modified_then_empty() {
    let both = book_with_dates("A", Some("2024-01-01"), Some("2024-06-01"));
    let modified_only = book_with_dates("B", None, Some("2024-06-01"));
    let neither = book_with_dates("C", None, None);
    assert_eq!(date_key(&both), "2024-01-01");
    assert_eq!(date_key(&modified_only), "2024-06-01");
    assert_eq!(date_key(&neither), "");
}

// ── ensure_fresh / sync_replica: TTL gate, paging, discard ─────────────

use axum::response::IntoResponse;

use crate::offline::sync::test_state_lock;
use crate::offline::{cache, store};

/// Bind a one-shot axum router on an ephemeral loopback port and return its
/// base URL — mirrors the `sync/tests.rs` `mock_server` helper.
async fn spawn_router(app: axum::Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://127.0.0.1:{port}")
}

/// Clear the replica's global cache row so each test starts from "no replica
/// synced yet", regardless of what an earlier test left behind.
async fn reset_replica_cache() {
    let st = store::store().expect("test store");
    st.kv_delete(&cache::keys::ebooks_all());
    // Land the delete before the test's own writes race it on the same
    // worker channel.
    let _ = st.kv_get(&cache::keys::ebooks_all()).await;
}

#[tokio::test]
async fn sync_replica_skips_refetch_when_the_cached_replica_is_still_fresh() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    reset_replica_cache().await;
    let st = store::store().expect("test store");
    let seeded = vec![book("Fresh", "Author", &["EPUB"])];
    st.kv_put(
        &cache::keys::ebooks_all(),
        serde_json::to_string(&seeded).expect("serialize"),
    );
    let _ = st.kv_get(&cache::keys::ebooks_all()).await;

    // If the TTL gate did not short-circuit, this server's distinct payload
    // would land in the cache instead of the seeded one.
    let app = axum::Router::new().route(
        "/api/ebooks",
        axum::routing::get(|| async {
            axum::Json(EbookLibrary {
                path: None,
                books: vec![book("ShouldNotAppear", "Other", &["EPUB"])],
                error: None,
                total: Some(1),
            })
        }),
    );
    let base = spawn_router(app).await;
    sync_replica(&base).await;

    let cached: Vec<EbookMetadata> = cache::get_json(&cache::keys::ebooks_all())
        .await
        .expect("cache untouched");
    assert_eq!(titles_of(&cached), vec!["Fresh"]);
}

fn titles_of(books: &[EbookMetadata]) -> Vec<String> {
    books
        .iter()
        .map(|b| b.title.clone().unwrap_or_default())
        .collect()
}

#[tokio::test]
async fn sync_replica_pages_through_the_cursor_loop_and_stores_the_full_replica() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    reset_replica_cache().await;

    let app = axum::Router::new().route(
        "/api/ebooks",
        axum::routing::get(
            |axum::extract::Query(params): axum::extract::Query<
                std::collections::HashMap<String, String>,
            >| async move {
                let mut headers = axum::http::HeaderMap::new();
                let books = match params.get("cursor").map(String::as_str) {
                    None => {
                        headers.insert("X-Next-Cursor", "p2".parse().unwrap());
                        vec![book("One", "A", &["EPUB"]), book("Two", "B", &["EPUB"])]
                    }
                    Some("p2") => vec![book("Three", "C", &["EPUB"])],
                    _ => vec![],
                };
                (
                    headers,
                    axum::Json(EbookLibrary {
                        path: None,
                        books,
                        error: None,
                        total: None,
                    }),
                )
            },
        ),
    );
    let base = spawn_router(app).await;
    sync_replica(&base).await;

    let cached: Vec<EbookMetadata> = cache::get_json(&cache::keys::ebooks_all())
        .await
        .expect("replica stored after a successful multi-page sync");
    let mut titles = titles_of(&cached);
    titles.sort();
    assert_eq!(titles, vec!["One", "Three", "Two"]);
}

#[tokio::test]
async fn sync_replica_discards_everything_when_a_later_page_fails() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    reset_replica_cache().await;

    let app = axum::Router::new().route(
        "/api/ebooks",
        axum::routing::get(
            |axum::extract::Query(params): axum::extract::Query<
                std::collections::HashMap<String, String>,
            >| async move {
                match params.get("cursor").map(String::as_str) {
                    None => {
                        let mut headers = axum::http::HeaderMap::new();
                        headers.insert("X-Next-Cursor", "p2".parse().unwrap());
                        (
                            axum::http::StatusCode::OK,
                            headers,
                            axum::Json(EbookLibrary {
                                path: None,
                                books: vec![book("Partial", "A", &["EPUB"])],
                                error: None,
                                total: None,
                            }),
                        )
                            .into_response()
                    }
                    _ => (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "boom").into_response(),
                }
            },
        ),
    );
    let base = spawn_router(app).await;
    sync_replica(&base).await;

    let cached: Option<Vec<EbookMetadata>> = cache::get_json(&cache::keys::ebooks_all()).await;
    assert!(
        cached.is_none(),
        "a mid-page failure must discard the partial page rather than caching it"
    );
}

#[test]
fn search_replica_matches_title_author_and_filename_case_insensitively() {
    let books = fixture();
    let by_title = search_replica(&books, "dUnE");
    assert_eq!(by_title.len(), 1);
    assert_eq!(by_title[0].title.as_deref(), Some("Dune"));

    let by_author = search_replica(&books, "morrison");
    assert_eq!(by_author.len(), 1);
    assert_eq!(by_author[0].title.as_deref(), Some("Beloved"));

    // filename contains "Atonement.epub"
    let by_file = search_replica(&books, "atonement.epub");
    assert_eq!(by_file.len(), 1);

    assert!(search_replica(&books, "   ").is_empty());
    assert!(search_replica(&books, "zzz-no-hit").is_empty());
}

// ── Hidden-formats exclusion over the replica ───────────────────────────

#[test]
fn visible_under_exclusion_keeps_dual_format_book_with_one_visible_format() {
    let dual = book("Dual", "A", &["CBZ", "EPUB"]);
    assert!(visible_under_exclusion(&dual, &["cbz".to_string()]));
    assert!(!visible_under_exclusion(
        &dual,
        &["cbz".to_string(), "epub".to_string()]
    ));
}

#[test]
fn visible_under_exclusion_keeps_formatless_physical_only_book() {
    let phys = book("Shelf Copy", "A", &[]);
    assert!(visible_under_exclusion(&phys, &["cbz".to_string()]));
}

#[test]
fn visible_under_exclusion_keeps_physically_owned_book_with_all_formats_hidden() {
    // The server's physical arm keeps an owned book visible even when every
    // file format is hidden; the offline rule must agree or a book flickers
    // out the moment the network drops.
    let mut owned = book("Owned Comic", "A", &["CBZ"]);
    owned.has_physical = true;
    assert!(visible_under_exclusion(&owned, &["cbz".to_string()]));
}

#[test]
fn visible_under_exclusion_hides_book_when_every_format_hidden() {
    // Stored formats are uppercase; the pref tokens lowercase.
    let comic = book("Comic", "A", &["CBZ"]);
    assert!(!visible_under_exclusion(&comic, &["cbz".to_string()]));
    assert!(visible_under_exclusion(&comic, &[]));
}

#[test]
fn page_from_replica_exclusion_hides_books_and_reports_first_page_receipt() {
    let mut books = fixture(); // four EPUB books
    books.push(book("Comic", "A", &["CBZ"]));

    let page = page_from_replica(
        books.clone(),
        SortKey::Title,
        SortDir::Asc,
        &[],
        &["cbz".to_string()],
        None,
        10,
    );
    assert_eq!(page.total, Some(4));
    assert_eq!(page.hidden_count, Some(1));
    assert!(titles(&page).iter().all(|t| t != "Comic"));

    // Later pages carry neither total nor receipt.
    let later = page_from_replica(
        books,
        SortKey::Title,
        SortDir::Asc,
        &[],
        &["cbz".to_string()],
        Some("off:2"),
        2,
    );
    assert_eq!(later.hidden_count, None);
}

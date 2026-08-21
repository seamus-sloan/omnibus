use super::{ebooks_page, merge_candidates, search_ebooks};
use omnibus_db::test_support::seed_synced_ebook;
use omnibus_shared::{Settings, SortDir, SortKey, ViewFilters, SEARCH_QUERY_MAX_LEN};

async fn configured_pool(audiobook_path: Option<&str>) -> sqlx::SqlitePool {
    let pool = omnibus_db::init_db("sqlite::memory:").await.unwrap();
    omnibus_db::set_settings(
        &pool,
        &Settings {
            ebook_library_path: Some("/ebooks".into()),
            audiobook_library_path: audiobook_path.map(Into::into),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    pool
}

#[tokio::test]
async fn ebooks_page_first_page_carries_total_and_facets() {
    let pool = configured_pool(None).await;
    seed_synced_ebook(&pool, "a.epub", "Alpha", "Ann Author").await;
    seed_synced_ebook(&pool, "b.epub", "Beta", "Bob Author").await;

    let first = ebooks_page(
        &pool,
        SortKey::Title,
        SortDir::Asc,
        &ViewFilters::default(),
        &[],
        None,
        1,
    )
    .await
    .unwrap();

    assert_eq!(first.books.len(), 1);
    assert_eq!(first.books[0].title.as_deref(), Some("Alpha"));
    assert_eq!(first.total, Some(2));
    assert!(first.facets.is_some());
    assert!(first.next_cursor.is_some());
}

#[tokio::test]
async fn ebooks_page_later_page_continues_after_cursor_and_omits_aggregates() {
    let pool = configured_pool(None).await;
    seed_synced_ebook(&pool, "a.epub", "Alpha", "Ann Author").await;
    seed_synced_ebook(&pool, "b.epub", "Beta", "Bob Author").await;

    let first = ebooks_page(
        &pool,
        SortKey::Title,
        SortDir::Asc,
        &ViewFilters::default(),
        &[],
        None,
        1,
    )
    .await
    .unwrap();
    let cursor = first.next_cursor.expect("first page should issue a cursor");

    let second = ebooks_page(
        &pool,
        SortKey::Title,
        SortDir::Asc,
        &ViewFilters::default(),
        &[],
        Some(&cursor),
        1,
    )
    .await
    .unwrap();

    assert_eq!(second.books.len(), 1);
    assert_eq!(second.books[0].title.as_deref(), Some("Beta"));
    assert_eq!(second.total, None, "later pages must omit the total");
    assert!(second.facets.is_none(), "later pages must omit the facets");
}

#[tokio::test]
async fn ebooks_page_surfaces_error_for_malformed_cursor() {
    let pool = configured_pool(None).await;

    let result = ebooks_page(
        &pool,
        SortKey::Title,
        SortDir::Asc,
        &ViewFilters::default(),
        &[],
        Some("not-a-server-issued-cursor"),
        10,
    )
    .await;

    assert!(result.is_err(), "malformed cursor must surface an error");
}

#[tokio::test]
async fn merge_candidates_dedups_shared_directory_hits_and_caps_at_twenty() {
    // Both library slots pointing at one directory is the documented
    // dedup case: every hit comes back once per path, so without the
    // uuid dedup the list would double.
    let pool = configured_pool(Some("/ebooks")).await;
    for i in 0..25 {
        seed_synced_ebook(
            &pool,
            &format!("tome-{i}.epub"),
            &format!("Common Tome {i}"),
            "Prolific Author",
        )
        .await;
    }

    let out = merge_candidates(&pool, "Common").await.unwrap();

    assert_eq!(out.len(), 20, "25 deduped hits must truncate to 20");
    let mut seen = std::collections::HashSet::new();
    assert!(
        out.iter().all(|b| seen.insert(b.unique_identifier.clone())),
        "no duplicate unique_identifier may survive the dedup"
    );
}

#[tokio::test]
async fn search_ebooks_rejects_query_over_the_length_cap() {
    let pool = configured_pool(None).await;
    let oversized = "a".repeat(SEARCH_QUERY_MAX_LEN + 1);

    let result = search_ebooks(&pool, &oversized).await;

    assert!(result.is_err(), "oversized query must be rejected");
}

#[tokio::test]
async fn merge_candidates_rejects_query_over_the_length_cap() {
    let pool = configured_pool(None).await;
    let oversized = "a".repeat(SEARCH_QUERY_MAX_LEN + 1);

    let result = merge_candidates(&pool, &oversized).await;

    assert!(result.is_err(), "oversized query must be rejected");
}

#[tokio::test]
async fn ebooks_page_with_exclusion_omits_hidden_books_and_reports_hidden_count() {
    let pool = configured_pool(None).await;
    seed_synced_ebook(&pool, "comic.cbz", "Comic", "Ann Author").await;
    seed_synced_ebook(&pool, "novel.epub", "Novel", "Bob Author").await;

    let page = ebooks_page(
        &pool,
        SortKey::Title,
        SortDir::Asc,
        &ViewFilters::default(),
        &["cbz".to_string()],
        None,
        50,
    )
    .await
    .unwrap();

    let titles: Vec<_> = page
        .books
        .iter()
        .filter_map(|b| b.title.as_deref())
        .collect();
    assert_eq!(titles, vec!["Novel"]);
    assert_eq!(page.hidden_count, Some(1));
}

#[tokio::test]
async fn ebooks_page_with_exclusion_reports_visible_total() {
    let pool = configured_pool(None).await;
    seed_synced_ebook(&pool, "comic.cbz", "Comic", "Ann Author").await;
    seed_synced_ebook(&pool, "novel.epub", "Novel", "Bob Author").await;

    let page = ebooks_page(
        &pool,
        SortKey::Title,
        SortDir::Asc,
        &ViewFilters::default(),
        &["cbz".to_string()],
        None,
        50,
    )
    .await
    .unwrap();

    assert_eq!(page.total, Some(1), "total is the visible library size");
}

#[tokio::test]
async fn ebooks_page_without_exclusion_keeps_current_total_and_no_hidden_count() {
    let pool = configured_pool(None).await;
    seed_synced_ebook(&pool, "comic.cbz", "Comic", "Ann Author").await;
    seed_synced_ebook(&pool, "novel.epub", "Novel", "Bob Author").await;

    let page = ebooks_page(
        &pool,
        SortKey::Title,
        SortDir::Asc,
        &ViewFilters::default(),
        &[],
        None,
        50,
    )
    .await
    .unwrap();

    assert_eq!(page.total, Some(2));
    assert_eq!(page.hidden_count, None);
}

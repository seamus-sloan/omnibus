//! Book identity and visibility: canonical-uuid resolution through
//! `merged_uuids`, scan-key lookups, the landing-projection sort index, and
//! the fileless physical-copy / wishlist visibility rule.

use crate::pool::init_db;
use crate::test_support::seed_minimal_books;

use super::super::*;

// ---------- bulk canonical-uuid resolve (issue #633) ----------

/// Insert a `merged_uuids` row the way the merge tx does, so the bulk resolver
/// has a merged-uuid to fall back through.
async fn seed_merged_uuid_for_bulk(pool: &sqlx::SqlitePool, uuid: &str, book_id: i64) {
    sqlx::query(
        "INSERT OR REPLACE INTO merged_uuids (uuid, book_id, format, library_path)
         VALUES (?, ?, 'epub', '/lib')",
    )
    .bind(uuid)
    .bind(book_id)
    .execute(pool)
    .await
    .expect("seed merged uuid");
}

#[tokio::test]
async fn resolve_canonical_book_uuids_bulk_maps_direct_merged_and_omits_unknown() {
    // Straddles all three cases in one call: a direct `books.uuid`, a merged
    // uuid that resolves to a different canonical, and an unknown uuid that
    // must be **absent** from the returned map (not present with `None`).
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 2).await;
    let books = list_books(&pool, "/lib").await.unwrap();
    let direct_uuid = books[0].unique_identifier.clone().unwrap();
    let survivor_id = books[1].id;
    let survivor_uuid = books[1].unique_identifier.clone().unwrap();
    seed_merged_uuid_for_bulk(&pool, "merged-uuid-x", survivor_id).await;

    let batch = vec![
        direct_uuid.clone(),
        "merged-uuid-x".to_string(),
        "no-such-uuid".to_string(),
        direct_uuid.clone(), // dup — bulk resolver must dedup without error
    ];
    let mut tx = pool.begin().await.unwrap();
    let map = resolve_canonical_book_uuids_bulk_exec(&mut tx, &batch)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(map.get(&direct_uuid), Some(&direct_uuid));
    assert_eq!(map.get("merged-uuid-x"), Some(&survivor_uuid));
    assert!(
        !map.contains_key("no-such-uuid"),
        "unknown uuids must be absent from the map (not None)"
    );
    assert_eq!(map.len(), 2, "dedup + skip-unknown: exactly two entries");
}

#[tokio::test]
async fn resolve_canonical_book_uuids_bulk_empty_input_returns_empty_map() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    let map = resolve_canonical_book_uuids_bulk_exec(&mut tx, &[])
        .await
        .unwrap();
    assert!(map.is_empty());
}

// ---------- migration 0025: library-scoped landing-sort index (F5) ----------

#[tokio::test]
async fn migration_creates_composite_library_sort_index_for_landing_projection() {
    // F5: the `(library_id, sort, id)` composite index lets the planner seek
    // the library filter and supply `ORDER BY sort, id` without a temp sort.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let exists: Option<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'index' AND name = 'idx_books_library_sort'",
    )
    .fetch_optional(&pool)
    .await
    .unwrap();
    assert_eq!(exists.as_deref(), Some("idx_books_library_sort"));
}

#[tokio::test]
async fn get_book_uuid_by_scan_key_returns_uuid_for_known_path() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 3).await;
    // `seed_minimal_books` files books under scan_root `/lib` with scan_key
    // `b<i>.epub` and uuid `uuid-<i>`.
    let uuid = get_book_uuid_by_scan_key(&pool, "/lib", "b2.epub")
        .await
        .unwrap();
    assert_eq!(uuid.as_deref(), Some("uuid-2"));
}

#[tokio::test]
async fn get_book_uuid_by_scan_key_returns_none_for_unknown_key() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    // Unknown scan_key under a known root, and a known key under an unknown
    // root, both resolve to None.
    assert!(get_book_uuid_by_scan_key(&pool, "/lib", "missing.epub")
        .await
        .unwrap()
        .is_none());
    assert!(get_book_uuid_by_scan_key(&pool, "/other", "b1.epub")
        .await
        .unwrap()
        .is_none());
}

// ---------- F Physical Check-In: physical-only visibility (#1181) ----------

async fn seed_fileless(pool: &sqlx::SqlitePool, title: &str, authors: Vec<&str>) -> String {
    crate::physical::create_fileless_book(
        pool,
        crate::physical::FilelessBook {
            title: title.into(),
            authors: authors.into_iter().map(str::to_string).collect(),
            isbn: None,
            pubdate: None,
            description: None,
            cover: None,
        },
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn physical_only_book_is_visible_but_wishlist_only_is_hidden() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await; // one normal /lib book

    // Physical-only: fileless book with a checked-in copy.
    let phys = seed_fileless(&pool, "Physical Only Title", vec!["Print Author"]).await;
    crate::physical::add_physical_copy(&pool, &phys, None, None, None)
        .await
        .unwrap();
    // Wishlist-only: fileless book, no physical copy.
    let wish = seed_fileless(&pool, "Wishlist Only Title", vec![]).await;

    let list = list_books(&pool, "/lib").await.unwrap();
    let uuids: Vec<&str> = list
        .iter()
        .filter_map(|b| b.unique_identifier.as_deref())
        .collect();
    assert!(
        uuids.contains(&phys.as_str()),
        "physical-only book must appear"
    );
    assert!(
        !uuids.contains(&wish.as_str()),
        "wishlist-only book must be hidden"
    );

    // AC3: the physical flag is set (drives the badge).
    let phys_row = list
        .iter()
        .find(|b| b.unique_identifier.as_deref() == Some(&phys))
        .unwrap();
    assert!(phys_row.has_physical);

    // AC1: searchable by title.
    let hits = search_books(&pool, "/lib", "Physical Only").await.unwrap();
    assert!(hits
        .iter()
        .any(|b| b.unique_identifier.as_deref() == Some(&phys)));
    // AC2: wishlist-only book is not searchable.
    let miss = search_books(&pool, "/lib", "Wishlist Only").await.unwrap();
    assert!(miss.is_empty());

    // AC1: the physical-only book contributes to sidebar facets even though it
    // lives under the synthetic `physical://local` root, not `/lib`.
    let facets = library_facets(&pool, &["/lib"]).await.unwrap();
    assert!(
        facets.authors.iter().any(|f| f.value == "Print Author"),
        "physical-only book's author must appear in facets"
    );
}

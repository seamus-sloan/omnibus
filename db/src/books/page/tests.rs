//! Keyset pagination tests for [`list_books_page`].

use std::sync::atomic::{AtomicU64, Ordering};

use omnibus_shared::{SortDir, SortKey, ViewFilters};
use sqlx::SqlitePool;

use super::*;
use crate::pool::init_db;
use crate::test_support::{seed_discovery_fixture, seed_minimal_books};

/// Unique uuid/scan_key per inserted row.
fn uniq() -> String {
    static N: AtomicU64 = AtomicU64::new(0);
    format!("uuid-{}", N.fetch_add(1, Ordering::Relaxed))
}

async fn insert_lib(pool: &SqlitePool, path: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO scan_roots (path, display_name) VALUES (?, 'lib') RETURNING id")
        .bind(path)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Insert a book with an explicit `(title, sort, series_sort, series_index)`
/// plus a backing `book_files` row (so the fileless filter keeps it). Returns the
/// new book id.
async fn insert_book(
    pool: &SqlitePool,
    lib_id: i64,
    title: &str,
    sort: Option<&str>,
    series_sort: Option<&str>,
    series_index: Option<f64>,
) -> i64 {
    let key = uniq();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, scan_key, library_id, path, title, sort, series_sort, series_index)
         VALUES (?, ?, ?, '/p', ?, ?, ?, ?) RETURNING id",
    )
    .bind(&key)
    .bind(&key)
    .bind(lib_id)
    .bind(title)
    .bind(sort)
    .bind(series_sort)
    .bind(series_index)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime_epoch)
         VALUES (?, 'EPUB', ?, 1, 1)",
    )
    .bind(id)
    .bind(title)
    .execute(pool)
    .await
    .unwrap();
    id
}

/// Like [`insert_book`], but with an explicit set of `book_files` formats
/// (stored-case, e.g. `"CBZ"`). An empty slice inserts no file rows — pair it
/// with [`insert_physical_copy`] or the fileless gate hides the book.
async fn insert_book_with_formats(
    pool: &SqlitePool,
    lib_id: i64,
    title: &str,
    formats: &[&str],
) -> i64 {
    let key = uniq();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, scan_key, library_id, path, title)
         VALUES (?, ?, ?, '/p', ?) RETURNING id",
    )
    .bind(&key)
    .bind(&key)
    .bind(lib_id)
    .bind(title)
    .fetch_one(pool)
    .await
    .unwrap();
    for (i, fmt) in formats.iter().enumerate() {
        sqlx::query(
            "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime_epoch, ordinal)
             VALUES (?, ?, ?, 1, 1, ?)",
        )
        .bind(id)
        .bind(fmt)
        .bind(format!("{title}.{}", fmt.to_lowercase()))
        .bind(i as i64)
        .execute(pool)
        .await
        .unwrap();
    }
    id
}

/// Attach a physical copy to a book so the physical OR-arm keeps it visible.
async fn insert_physical_copy(pool: &SqlitePool, book_id: i64) {
    let uuid: String = sqlx::query_scalar("SELECT uuid FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_one(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO physical_copies (book_uuid) VALUES (?)")
        .bind(uuid)
        .execute(pool)
        .await
        .unwrap();
}

fn ids(page: &BookPage) -> Vec<i64> {
    page.books.iter().map(|b| b.id).collect()
}

fn titles(page: &BookPage) -> Vec<String> {
    page.books
        .iter()
        .map(|b| b.title.clone().unwrap_or_default())
        .collect()
}

// ---------------------------------------------------------------------------
// Acceptance: the gating test — paging walks the whole ordered set, gap-free.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_books_page_returns_first_then_next_page_via_cursor_with_no_overlap() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 7).await; // sorts 'Title 0000000001'..'..7'
    let f = ViewFilters::default();

    let p1 = list_books_page(
        &pool,
        &["/lib"],
        SortKey::Title,
        SortDir::Asc,
        &f,
        &[],
        None,
        3,
    )
    .await
    .unwrap();
    assert_eq!(p1.books.len(), 3, "page 1 is exactly the page size");
    assert!(p1.next.is_some(), "more rows remain after page 1");

    let p2 = list_books_page(
        &pool,
        &["/lib"],
        SortKey::Title,
        SortDir::Asc,
        &f,
        &[],
        p1.next.as_ref(),
        3,
    )
    .await
    .unwrap();
    assert_eq!(p2.books.len(), 3);
    assert!(p2.next.is_some());

    let p3 = list_books_page(
        &pool,
        &["/lib"],
        SortKey::Title,
        SortDir::Asc,
        &f,
        &[],
        p2.next.as_ref(),
        3,
    )
    .await
    .unwrap();
    assert_eq!(p3.books.len(), 1, "last page is the remainder");
    assert!(p3.next.is_none(), "no cursor at end of stream");

    let mut all = ids(&p1);
    all.extend(ids(&p2));
    all.extend(ids(&p3));
    let unique: std::collections::HashSet<i64> = all.iter().copied().collect();
    assert_eq!(unique.len(), 7, "pages are disjoint and cover the full set");
    assert_eq!(all, (1..=7).collect::<Vec<_>>(), "global title-asc order");
}

#[tokio::test]
async fn list_books_page_handles_null_sort_rows_at_first_page_boundary() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = insert_lib(&pool, "/lib").await;
    // Two NULL-sort rows (collate first under ASC) + three present.
    let n1 = insert_book(&pool, lib, "zeta", None, None, None).await;
    let n2 = insert_book(&pool, lib, "yarn", None, None, None).await;
    let a = insert_book(&pool, lib, "Alpha", Some("Alpha"), None, None).await;
    let b = insert_book(&pool, lib, "Beta", Some("Beta"), None, None).await;
    let c = insert_book(&pool, lib, "Gamma", Some("Gamma"), None, None).await;
    let f = ViewFilters::default();

    // Walk the whole library two at a time; assert the NULL-sort rows lead
    // (by id) and nothing is skipped or duplicated across the boundary.
    let mut seen = Vec::new();
    let mut cursor: Option<PageCursor> = None;
    loop {
        let page = list_books_page(
            &pool,
            &["/lib"],
            SortKey::Title,
            SortDir::Asc,
            &f,
            &[],
            cursor.as_ref(),
            2,
        )
        .await
        .unwrap();
        seen.extend(ids(&page));
        match page.next {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    assert_eq!(
        seen,
        vec![n1, n2, a, b, c],
        "nulls first (by id), then sorted"
    );
}

#[tokio::test]
async fn newest_added_orders_by_epoch_and_paginates_via_cursor() {
    // `books.timestamp` is INTEGER unix-seconds (migration 0038); the axis
    // formats it to fixed-width ISO so the keyset cursor round-trips it as text
    // and still sorts chronologically. Distinct epochs + one-at-a-time paging
    // exercise both the ordering and the cursor across page boundaries.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = insert_lib(&pool, "/lib").await;
    let a = insert_book(&pool, lib, "A", Some("A"), None, None).await;
    let b = insert_book(&pool, lib, "B", Some("B"), None, None).await;
    let c = insert_book(&pool, lib, "C", Some("C"), None, None).await;
    for (id, day) in [(a, "2020-01-01"), (b, "2022-06-15"), (c, "2024-12-31")] {
        sqlx::query("UPDATE books SET timestamp = strftime('%s', ? || ' 00:00:00') WHERE id = ?")
            .bind(day)
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
    }
    let f = ViewFilters::default();

    // Newest first: c, b, a.
    let page = list_books_page(
        &pool,
        &["/lib"],
        SortKey::NewestAdded,
        SortDir::Desc,
        &f,
        &[],
        None,
        50,
    )
    .await
    .unwrap();
    assert_eq!(ids(&page), vec![c, b, a]);

    // Paging one at a time reproduces the same order with no gaps/dupes.
    let mut seen = Vec::new();
    let mut cursor: Option<PageCursor> = None;
    loop {
        let p = list_books_page(
            &pool,
            &["/lib"],
            SortKey::NewestAdded,
            SortDir::Desc,
            &f,
            &[],
            cursor.as_ref(),
            1,
        )
        .await
        .unwrap();
        seen.extend(ids(&p));
        match p.next {
            Some(n) => cursor = Some(n),
            None => break,
        }
    }
    assert_eq!(seen, vec![c, b, a]);
}

#[tokio::test]
async fn list_books_page_breaks_sort_ties_by_id_deterministically() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = insert_lib(&pool, "/lib").await;
    let mut expected = Vec::new();
    for _ in 0..5 {
        expected.push(insert_book(&pool, lib, "Same", Some("Same"), None, None).await);
    }
    let f = ViewFilters::default();

    let mut seen = Vec::new();
    let mut cursor: Option<PageCursor> = None;
    loop {
        let page = list_books_page(
            &pool,
            &["/lib"],
            SortKey::Title,
            SortDir::Asc,
            &f,
            &[],
            cursor.as_ref(),
            2,
        )
        .await
        .unwrap();
        seen.extend(ids(&page));
        match page.next {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    assert_eq!(seen, expected, "identical sort keys order by id, gap-free");
}

#[tokio::test]
async fn list_books_page_respects_library_path_filter() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib_a = insert_lib(&pool, "/a").await;
    let lib_b = insert_lib(&pool, "/b").await;
    for t in ["a1", "a2", "a3"] {
        insert_book(&pool, lib_a, t, Some(t), None, None).await;
    }
    let b_ids: Vec<i64> = {
        let mut v = Vec::new();
        for t in ["b1", "b2"] {
            v.push(insert_book(&pool, lib_b, t, Some(t), None, None).await);
        }
        v
    };
    let f = ViewFilters::default();

    let mut seen = Vec::new();
    let mut cursor: Option<PageCursor> = None;
    loop {
        let page = list_books_page(
            &pool,
            &["/a"],
            SortKey::Title,
            SortDir::Asc,
            &f,
            &[],
            cursor.as_ref(),
            2,
        )
        .await
        .unwrap();
        seen.extend(ids(&page));
        match page.next {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    assert_eq!(seen.len(), 3, "only library A rows");
    assert!(
        b_ids.iter().all(|id| !seen.contains(id)),
        "a cursor scoped to /a never returns /b rows"
    );
}

#[tokio::test]
async fn list_books_page_returns_empty_at_end_of_stream() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 2).await;
    let f = ViewFilters::default();

    // A full read under a generous limit ends with no cursor.
    let all = list_books_page(
        &pool,
        &["/lib"],
        SortKey::Title,
        SortDir::Asc,
        &f,
        &[],
        None,
        50,
    )
    .await
    .unwrap();
    assert_eq!(all.books.len(), 2);
    assert!(all.next.is_none());

    // A cursor positioned past the last row yields nothing and no next.
    let past = PageCursor {
        sort: Some("zzzzzzzzzz".into()),
        idx: None,
        id: i64::MAX,
    };
    let empty = list_books_page(
        &pool,
        &["/lib"],
        SortKey::Title,
        SortDir::Asc,
        &f,
        &[],
        Some(&past),
        50,
    )
    .await
    .unwrap();
    assert!(empty.books.is_empty());
    assert!(empty.next.is_none());
}

#[tokio::test]
async fn list_books_page_paginates_descending_without_overlap() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 5).await;
    let f = ViewFilters::default();

    let mut seen = Vec::new();
    let mut cursor: Option<PageCursor> = None;
    loop {
        let page = list_books_page(
            &pool,
            &["/lib"],
            SortKey::Title,
            SortDir::Desc,
            &f,
            &[],
            cursor.as_ref(),
            2,
        )
        .await
        .unwrap();
        seen.extend(ids(&page));
        match page.next {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    assert_eq!(seen, vec![5, 4, 3, 2, 1], "title-desc order, gap-free");
}

// ---------------------------------------------------------------------------
// Series axis — the denormalized series_sort + series_index keyset.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_books_page_sorts_series_axis_by_name_then_index() {
    let (pool, _guard) = seed_discovery_fixture().await;
    let f = ViewFilters::default();
    let page = list_books_page(
        &pool,
        &["/lib"],
        SortKey::Series,
        SortDir::Asc,
        &f,
        &[],
        None,
        50,
    )
    .await
    .unwrap();
    // Seriesless first (NULL series_sort collates first under ASC), then
    // Pioneers, then Saga #1 before Saga #2 (series_index tiebreak).
    assert_eq!(
        titles(&page),
        vec![
            "Standalone",
            "Other Story",
            "Saga: Book One",
            "Saga: Book Two",
        ]
    );
}

#[tokio::test]
async fn list_books_page_paginates_series_axis_with_cursor_no_overlap() {
    let (pool, _guard) = seed_discovery_fixture().await;
    let f = ViewFilters::default();

    let mut seen = Vec::new();
    let mut cursor: Option<PageCursor> = None;
    loop {
        let page = list_books_page(
            &pool,
            &["/lib"],
            SortKey::Series,
            SortDir::Asc,
            &f,
            &[],
            cursor.as_ref(),
            1,
        )
        .await
        .unwrap();
        seen.extend(titles(&page));
        match page.next {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    assert_eq!(
        seen,
        vec![
            "Standalone",
            "Other Story",
            "Saga: Book One",
            "Saga: Book Two",
        ],
        "single-row pages reassemble the full series order with no overlap"
    );
}

// ---------------------------------------------------------------------------
// Server-side filters.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_books_page_applies_author_filter_server_side() {
    let (pool, _guard) = seed_discovery_fixture().await;
    let f = ViewFilters {
        authors: vec!["Ada Lovelace".into()],
        ..Default::default()
    };
    let page = list_books_page(
        &pool,
        &["/lib"],
        SortKey::Title,
        SortDir::Asc,
        &f,
        &[],
        None,
        50,
    )
    .await
    .unwrap();
    assert_eq!(page.books.len(), 3, "Ada wrote three of the four fixtures");
    assert!(page
        .books
        .iter()
        .all(|b| b.creators.iter().any(|c| c.name == "Ada Lovelace")));
}

#[tokio::test]
async fn list_books_page_applies_format_filter_case_insensitively() {
    let (pool, _guard) = seed_discovery_fixture().await; // all EPUB
    let epub = ViewFilters {
        formats: vec!["epub".into()],
        ..Default::default()
    };
    let page = list_books_page(
        &pool,
        &["/lib"],
        SortKey::Title,
        SortDir::Asc,
        &epub,
        &[],
        None,
        50,
    )
    .await
    .unwrap();
    assert_eq!(page.books.len(), 4, "lowercase chip matches stored EPUB");

    let m4b = ViewFilters {
        formats: vec!["m4b".into()],
        ..Default::default()
    };
    let none = list_books_page(
        &pool,
        &["/lib"],
        SortKey::Title,
        SortDir::Asc,
        &m4b,
        &[],
        None,
        50,
    )
    .await
    .unwrap();
    assert!(none.books.is_empty(), "no audiobooks in the fixture");
}

#[tokio::test]
async fn list_books_page_returns_empty_for_no_paths() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let f = ViewFilters::default();
    let page = list_books_page(&pool, &[], SortKey::Title, SortDir::Asc, &f, &[], None, 50)
        .await
        .unwrap();
    assert!(page.books.is_empty());
    assert!(page.next.is_none());
}

// ---------------------------------------------------------------------------
// Cursor encoding.
// ---------------------------------------------------------------------------

#[test]
fn page_cursor_encode_decode_roundtrips() {
    let series = PageCursor {
        sort: Some("Foundation".into()),
        idx: Some(1.5),
        id: 42,
    };
    assert_eq!(PageCursor::decode(&series.encode()).unwrap(), series);

    let null = PageCursor {
        sort: None,
        idx: None,
        id: 7,
    };
    assert_eq!(PageCursor::decode(&null.encode()).unwrap(), null);
}

#[test]
fn page_cursor_encode_coerces_non_finite_idx_instead_of_emitting_empty() {
    // A NaN/Inf series_index must not make `encode` silently return "" (which
    // would break the next page fetch) — it's coerced to None and round-trips.
    let c = PageCursor {
        sort: Some("Saga".into()),
        idx: Some(f64::NAN),
        id: 3,
    };
    let encoded = c.encode();
    assert!(
        !encoded.is_empty(),
        "encode must never produce an empty cursor"
    );
    let decoded = PageCursor::decode(&encoded).expect("non-finite idx still decodes");
    assert_eq!(decoded.idx, None);
    assert_eq!(decoded.sort.as_deref(), Some("Saga"));
    assert_eq!(decoded.id, 3);
}

#[test]
fn page_cursor_decode_rejects_malformed() {
    // Not base64url.
    assert!(matches!(
        PageCursor::decode("@@@not-base64@@@"),
        Err(CursorError::Malformed)
    ));
    // Valid base64 of "abc", which is not the cursor JSON shape.
    assert!(matches!(
        PageCursor::decode("YWJj"),
        Err(CursorError::Malformed)
    ));
}

// ---------------------------------------------------------------------------
// F Physical Check-In: physical-only visibility (#1181) on the landing/browse
// path — mirrors `physical_only_book_is_visible_but_wishlist_only_is_hidden`
// in `crate::books::tests`, which covers the equivalent `list.rs` path.
// ---------------------------------------------------------------------------

async fn seed_fileless(pool: &SqlitePool, title: &str, authors: Vec<&str>) -> String {
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
async fn list_books_page_shows_physical_only_book_but_hides_wishlist_only() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await; // one normal /lib book

    // Physical-only: fileless book with a checked-in copy.
    let phys = seed_fileless(&pool, "Physical Only Title", vec!["Print Author"]).await;
    crate::physical::add_physical_copy(&pool, &phys, None, None, None)
        .await
        .unwrap();
    // Wishlist-only: fileless book, no physical copy.
    let wish = seed_fileless(&pool, "Wishlist Only Title", vec![]).await;

    let f = ViewFilters::default();
    let page = list_books_page(
        &pool,
        &["/lib"],
        SortKey::Title,
        SortDir::Asc,
        &f,
        &[],
        None,
        50,
    )
    .await
    .unwrap();
    let uuids: Vec<&str> = page
        .books
        .iter()
        .filter_map(|b| b.unique_identifier.as_deref())
        .collect();
    assert!(
        uuids.contains(&phys.as_str()),
        "physical-only book must appear in the browse page"
    );
    assert!(
        !uuids.contains(&wish.as_str()),
        "wishlist-only book must be hidden from the browse page"
    );

    let phys_row = page
        .books
        .iter()
        .find(|b| b.unique_identifier.as_deref() == Some(&phys))
        .unwrap();
    assert!(phys_row.has_physical, "physical flag drives the badge");
}

#[tokio::test]
async fn count_books_page_counts_physical_only_book_but_not_wishlist_only() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await; // one normal /lib book

    let phys = seed_fileless(&pool, "Physical Only Title", vec!["Print Author"]).await;
    crate::physical::add_physical_copy(&pool, &phys, None, None, None)
        .await
        .unwrap();
    seed_fileless(&pool, "Wishlist Only Title", vec![]).await;

    // 1 normal book + 1 physical-only book; the wishlist-only book is excluded.
    let count = count_books_page(&pool, &["/lib"], &ViewFilters::default(), &[])
        .await
        .unwrap();
    assert_eq!(
        count, 2,
        "count includes physical-only but not wishlist-only"
    );
}

#[tokio::test]
async fn count_books_page_matches_unfiltered_count_and_applies_format_filter() {
    let (pool, _guard) = seed_discovery_fixture().await; // all EPUB
    let all = count_books_page(&pool, &["/lib"], &ViewFilters::default(), &[])
        .await
        .unwrap();
    assert_eq!(all, 4, "empty filters count the whole library");

    let epub = ViewFilters {
        formats: vec!["epub".into()],
        ..Default::default()
    };
    assert_eq!(
        count_books_page(&pool, &["/lib"], &epub, &[])
            .await
            .unwrap(),
        4
    );

    let m4b = ViewFilters {
        formats: vec!["m4b".into()],
        ..Default::default()
    };
    assert_eq!(
        count_books_page(&pool, &["/lib"], &m4b, &[]).await.unwrap(),
        0
    );

    // No library paths → zero without touching the db.
    assert_eq!(
        count_books_page(&pool, &[], &ViewFilters::default(), &[])
            .await
            .unwrap(),
        0
    );
}

// ---------------------------------------------------------------------------
// Hidden-formats exclusion (the landing-only `exclude_formats` predicate).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_books_page_excludes_books_whose_every_format_is_hidden() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = insert_lib(&pool, "/lib").await;
    insert_book_with_formats(&pool, lib, "Berserk Vol 1", &["CBZ"]).await;
    insert_book_with_formats(&pool, lib, "Plain Novel", &["EPUB"]).await;

    let f = ViewFilters::default();
    let hide = vec!["cbz".to_string()];
    let page = list_books_page(
        &pool,
        &["/lib"],
        SortKey::Title,
        SortDir::Asc,
        &f,
        &hide,
        None,
        50,
    )
    .await
    .unwrap();
    assert_eq!(titles(&page), vec!["Plain Novel"]);
}

#[tokio::test]
async fn list_books_page_keeps_dual_format_book_while_any_format_stays_visible() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = insert_lib(&pool, "/lib").await;
    insert_book_with_formats(&pool, lib, "Dual", &["CBZ", "EPUB"]).await;
    insert_book_with_formats(&pool, lib, "Comic Only", &["CBZ"]).await;

    let f = ViewFilters::default();
    let hide = vec!["cbz".to_string()];
    let page = list_books_page(
        &pool,
        &["/lib"],
        SortKey::Title,
        SortDir::Asc,
        &f,
        &hide,
        None,
        50,
    )
    .await
    .unwrap();
    assert_eq!(
        titles(&page),
        vec!["Dual"],
        "epub side keeps the book visible"
    );

    // Hiding every format the book carries finally hides it.
    let hide_both = vec!["cbz".to_string(), "epub".to_string()];
    let page = list_books_page(
        &pool,
        &["/lib"],
        SortKey::Title,
        SortDir::Asc,
        &f,
        &hide_both,
        None,
        50,
    )
    .await
    .unwrap();
    assert!(titles(&page).is_empty());
}

#[tokio::test]
async fn list_books_page_never_hides_physical_only_books() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = insert_lib(&pool, "/lib").await;
    // Physical-only: no book_files rows at all.
    let phys = insert_book_with_formats(&pool, lib, "Shelf Copy", &[]).await;
    insert_physical_copy(&pool, phys).await;
    // All-hidden files plus a physical copy: physical ownership trumps hiding.
    let dual = insert_book_with_formats(&pool, lib, "Hidden But Owned", &["CBZ"]).await;
    insert_physical_copy(&pool, dual).await;

    let f = ViewFilters::default();
    let hide = vec!["cbz".to_string()];
    let page = list_books_page(
        &pool,
        &["/lib"],
        SortKey::Title,
        SortDir::Asc,
        &f,
        &hide,
        None,
        50,
    )
    .await
    .unwrap();
    // NULL `sort` rows order by insertion id, so Shelf Copy (inserted first)
    // leads — the assertion is about visibility, not order.
    assert_eq!(titles(&page), vec!["Shelf Copy", "Hidden But Owned"]);
}

#[tokio::test]
async fn list_books_page_exclusion_matches_stored_uppercase_formats_case_insensitively() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = insert_lib(&pool, "/lib").await;
    // Stored uppercase (the scanner's convention); wire tokens are lowercase.
    insert_book_with_formats(&pool, lib, "Comic", &["CBZ"]).await;

    let f = ViewFilters::default();
    let hide = vec!["cbz".to_string()];
    let page = list_books_page(
        &pool,
        &["/lib"],
        SortKey::Title,
        SortDir::Asc,
        &f,
        &hide,
        None,
        50,
    )
    .await
    .unwrap();
    assert!(
        titles(&page).is_empty(),
        "lowercase 'cbz' must match stored 'CBZ' via COLLATE NOCASE"
    );
}

#[tokio::test]
async fn list_books_page_exclusion_composes_with_include_format_filter() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = insert_lib(&pool, "/lib").await;
    insert_book_with_formats(&pool, lib, "Comic", &["CBZ"]).await;
    insert_book_with_formats(&pool, lib, "Novel", &["EPUB"]).await;
    insert_book_with_formats(&pool, lib, "Audio", &["M4B"]).await;

    // Include-filter selects cbz+epub; exclusion hides cbz. The include list
    // must not resurrect a hidden book ("All" chip semantics).
    let f = ViewFilters {
        formats: vec!["cbz".into(), "epub".into()],
        ..Default::default()
    };
    let hide = vec!["cbz".to_string()];
    let page = list_books_page(
        &pool,
        &["/lib"],
        SortKey::Title,
        SortDir::Asc,
        &f,
        &hide,
        None,
        50,
    )
    .await
    .unwrap();
    assert_eq!(titles(&page), vec!["Novel"]);
}

#[tokio::test]
async fn count_books_page_exclusion_matches_list_books_page() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = insert_lib(&pool, "/lib").await;
    insert_book_with_formats(&pool, lib, "Comic A", &["CBZ"]).await;
    insert_book_with_formats(&pool, lib, "Comic B", &["CBZ"]).await;
    insert_book_with_formats(&pool, lib, "Dual", &["CBZ", "EPUB"]).await;
    insert_book_with_formats(&pool, lib, "Novel", &["EPUB"]).await;

    let f = ViewFilters::default();
    let hide = vec!["cbz".to_string()];
    let page = list_books_page(
        &pool,
        &["/lib"],
        SortKey::Title,
        SortDir::Asc,
        &f,
        &hide,
        None,
        50,
    )
    .await
    .unwrap();
    let count = count_books_page(&pool, &["/lib"], &f, &hide).await.unwrap();
    assert_eq!(page.books.len() as i64, count);
    assert_eq!(count, 2, "Dual + Novel");

    // The receipt arithmetic: same filters, exclusion diff only.
    let all = count_books_page(&pool, &["/lib"], &f, &[]).await.unwrap();
    assert_eq!(all - count, 2, "two comic-only books hidden");
}

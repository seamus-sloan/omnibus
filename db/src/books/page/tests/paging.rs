//! Walking the ordered set: first-then-next pages via the cursor with no
//! overlap or gaps, NULL sort rows at a page boundary, the epoch-ordered
//! newest-added axis, deterministic id tie-breaks, descending order, the
//! end of the stream, the series axis, and `PageCursor` encoding.

use omnibus_shared::{SortDir, SortKey, ViewFilters};

use super::super::*;
use super::{ids, insert_book, insert_lib, titles};
use crate::pool::init_db;
use crate::test_support::{seed_discovery_fixture, seed_minimal_books};

// Acceptance: the gating test — paging walks the whole ordered set, gap-free.
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

// Series axis — the denormalized series_sort + series_index keyset.
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

// Cursor encoding.
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

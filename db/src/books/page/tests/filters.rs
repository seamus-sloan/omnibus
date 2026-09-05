//! Server-side filters and visibility: the library-path, author and
//! format filters, the empty path list, physical-only books shown while
//! wishlist-only stay hidden (list and count agreeing), and the
//! hidden-formats exclusion composing with the include filter.

use omnibus_shared::{SortDir, SortKey, ViewFilters};
use sqlx::SqlitePool;

use super::super::*;
use super::{ids, insert_book, insert_lib, titles, uniq};
use crate::pool::init_db;
use crate::test_support::{seed_discovery_fixture, seed_minimal_books};

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

// Server-side filters.
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

// F Physical Check-In: physical-only visibility (#1181) on the landing/browse
// path — mirrors `physical_only_book_is_visible_but_wishlist_only_is_hidden`
// in `crate::books::tests`, which covers the equivalent `list.rs` path.
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

// Hidden-formats exclusion (the landing-only `exclude_formats` predicate).
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

//! Metadata overrides in the palette: a book hit carries the overridden
//! title, author display and cover, and the author, tag and series arms
//! count (and display) against the overridden values rather than the
//! scanned ones.

use omnibus_shared::{Contributor, MetadataOverrides};

use super::super::*;
use crate::books::list_books;
use crate::metadata_overrides::upsert_metadata_overrides;
use crate::pool::init_db;
use crate::sync::replace_books;
use crate::test_support::{indexed, CoversTempDir};

/// Bug #1 (display side): the palette must show the overridden title,
/// not the canonical scanned `b.title`, so what the user clicks matches
/// what they searched for.
#[tokio::test]
async fn search_palette_book_hit_uses_overridden_title() {
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
async fn search_palette_book_hit_uses_overridden_author_display() {
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
async fn search_palette_book_hit_uses_overridden_cover() {
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

#[tokio::test]
async fn search_palette_author_count_reflects_overrides() {
    // F5.1: the palette author count must match the merged
    // (override-aware) view, not the raw `books_authors_link` count.
    // Repro of the "Sanderson, Brandon still says 4 books" report:
    // every canonical book for an author was reassigned to a
    // differently-named author through the metadata edit form.
    //
    // #2502 tightened what happens to the *source* row. This test used to
    // assert it stayed listed reporting 0 books; that is the dead end the
    // issue is about — the row opens onto an author page reading IN YOUR
    // LIBRARY 0 — so it is now required to be absent, and only the
    // destination count survives as the F5.1 assertion.
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

    // The source row is credited with nothing now, so it is not offered at
    // all (#2502) — a canonical link the file still carries is not a reason
    // to advertise an author the library no longer shows.
    assert!(
        !results.authors.iter().any(|a| a.name == "Last, First"),
        "renamed-away author must not be offered, got {results:?}",
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
async fn search_palette_tag_count_reflects_overrides() {
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
async fn search_palette_series_count_reflects_overrides() {
    // F5.1: same shape as search_palette_author_count_reflects_overrides
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
async fn search_palette_series_author_display_reflects_override() {
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

// --- dead scanned author rows (#2502) -----------------------------------

/// Seed one book whose scanned creator is the file's inverted "Last, First"
/// form and whose displayed creator is an override, then return the pool.
/// This is the shape that leaves a scanned `authors` row credited with
/// nothing.
async fn seed_recredited_book(covers: &str) -> sqlx::SqlitePool {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "crows.epub",
            Some("Six of Crows"),
            &["Bardugo, Leigh"],
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
            creators: Some(vec![Contributor {
                name: "Leigh Bardugo".into(),
                ..Default::default()
            }]),
            ..Default::default()
        },
        false,
        user_id,
    )
    .await
    .unwrap();
    let _ = covers;
    pool
}

// Regression for #2502: the scanned "Bardugo, Leigh" row survived on a bare
// canonical-link EXISTS and was offered as `0 books · incl. Six of Crows`,
// opening onto an author page reading IN YOUR LIBRARY 0.
#[tokio::test]
async fn search_palette_authors_arm_drops_a_row_credited_with_no_books() {
    let _covers = CoversTempDir::new("palette_dead_author_row");
    let pool = seed_recredited_book("palette_dead_author_row").await;

    let palette = search_palette(&pool, "/lib", "Bardugo").await.unwrap();

    // Exactly one author, the one the book detail page links to, and it is
    // credited with the book.
    assert_eq!(
        palette.authors.len(),
        1,
        "expected only the credited author, got {:?}",
        palette.authors
    );
    assert_eq!(palette.authors[0].name, "Leigh Bardugo");
    assert_eq!(palette.authors[0].book_count, 1);
    assert_eq!(
        palette.authors[0].lead_book_title.as_deref(),
        Some("Six of Crows")
    );
}

// AC2's other half: the uncapped total behind the 5-hit cap has to count the
// same rows the list returns, or the rail promises hits that don't exist.
#[tokio::test]
async fn count_authors_agrees_with_the_rows_the_authors_arm_returns() {
    let _covers = CoversTempDir::new("palette_dead_author_count");
    let pool = seed_recredited_book("palette_dead_author_count").await;

    let total = count_authors(&pool, "/lib", "%Bardugo%").await.unwrap();
    let hits = search_authors(&pool, "/lib", "%Bardugo%", 10)
        .await
        .unwrap();

    assert_eq!(total, 1);
    assert_eq!(hits.len(), 1);
    assert_eq!(i64::try_from(hits.len()).unwrap(), total);
}

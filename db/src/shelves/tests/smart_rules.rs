//! Smart-shelf tag and genre rules against overrides and physical-only
//! books: override-added subjects and genres match, scanned tags replaced
//! by an override do not, a fileless book counts only with a physical
//! copy, and Kobo sync excludes physical-only members.

use omnibus_shared::{
    MatchMode, RuleField, RuleOp, ShelfKind, ShelfRule, SortDir, SortKey, UpdateShelfRequest,
};

use super::super::*;
use super::{make_user, smart_req, tag_rule, uuid_by_title};
use crate::pool::init_db;
use crate::test_support::seed_discovery_fixture;

#[tokio::test]
async fn create_smart_shelf_membership_matches_tag_rule() {
    let (pool, _covers) = seed_discovery_fixture().await;
    let owner = make_user(&pool, "owner", false).await;

    // Fixture: two books tagged "fiction" (Saga #1 and #2).
    let shelf = create_shelf(
        &pool,
        owner,
        &smart_req("Fiction", MatchMode::Any, vec![tag_rule("fiction")]),
    )
    .await
    .unwrap();
    assert_eq!(shelf.kind, ShelfKind::Smart);
    assert_eq!(shelf.book_count, 2);

    let page = shelf_page(&pool, &shelf, SortKey::Title, SortDir::Asc)
        .await
        .unwrap();
    assert_eq!(page.books.len(), 2);
    assert!(page
        .books
        .iter()
        .all(|b| b.subjects.iter().any(|s| s == "fiction")));
}

#[tokio::test]
async fn smart_shelf_tag_rule_matches_override_added_subjects() {
    let (pool, _covers) = seed_discovery_fixture().await;
    let owner = make_user(&pool, "owner", false).await;

    // "Other Story" is scanned as "nonfiction"; the user retags it through a
    // subjects override, so the tag exists only in the override JSON — never
    // in `books_tags_link` (see `materialize_tag_rows`).
    let uuid = uuid_by_title(&pool, "Other Story").await;
    let overrides = omnibus_shared::MetadataOverrides {
        subjects: Some(vec!["Seamus".into()]),
        ..Default::default()
    };
    crate::upsert_metadata_overrides(&pool, &uuid, &overrides, false, owner)
        .await
        .unwrap();

    // `starts with` and case-insensitive `is` both reach the override arm.
    let prefix = ShelfRule {
        field: RuleField::Tag,
        op: RuleOp::StartsWith,
        value: "Sea".into(),
    };
    let shelf = create_shelf(
        &pool,
        owner,
        &smart_req("Seamus Books", MatchMode::Any, vec![prefix]),
    )
    .await
    .unwrap();
    assert_eq!(shelf.book_count, 1);

    let shelf = create_shelf(
        &pool,
        owner,
        &smart_req("Seamus Eq", MatchMode::Any, vec![tag_rule("seamus")]),
    )
    .await
    .unwrap();
    assert_eq!(shelf.book_count, 1);
}

#[tokio::test]
async fn smart_shelf_genre_rule_matches_override_genres_only() {
    let (pool, _covers) = seed_discovery_fixture().await;
    let owner = make_user(&pool, "owner", false).await;

    // Genres are override-only (migration 0066): assign one to a single
    // book, then a genre rule must match exactly that book — never a book
    // whose *tags* carry the same word.
    let uuid = uuid_by_title(&pool, "Other Story").await;
    let overrides = omnibus_shared::MetadataOverrides {
        genres: Some(vec!["Space Opera".into()]),
        ..Default::default()
    };
    crate::upsert_metadata_overrides(&pool, &uuid, &overrides, false, owner)
        .await
        .unwrap();

    // Case-insensitive equality and prefix both resolve the override array.
    let eq = ShelfRule {
        field: RuleField::Genre,
        op: RuleOp::Is,
        value: "space opera".into(),
    };
    let shelf = create_shelf(&pool, owner, &smart_req("Opera", MatchMode::Any, vec![eq]))
        .await
        .unwrap();
    assert_eq!(shelf.book_count, 1);

    let prefix = ShelfRule {
        field: RuleField::Genre,
        op: RuleOp::StartsWith,
        value: "Space".into(),
    };
    let shelf = create_shelf(
        &pool,
        owner,
        &smart_req("Opera Prefix", MatchMode::Any, vec![prefix]),
    )
    .await
    .unwrap();
    assert_eq!(shelf.book_count, 1);

    // "fiction" exists as a *tag* on two books but as a genre on none — the
    // genre field must not fall back to the tag taxonomy.
    let tag_word = ShelfRule {
        field: RuleField::Genre,
        op: RuleOp::Is,
        value: "fiction".into(),
    };
    let shelf = create_shelf(
        &pool,
        owner,
        &smart_req("Not Tags", MatchMode::Any, vec![tag_word]),
    )
    .await
    .unwrap();
    assert_eq!(shelf.book_count, 0);
}

#[tokio::test]
async fn smart_shelf_tag_rule_skips_scanned_tags_replaced_by_override() {
    let (pool, _covers) = seed_discovery_fixture().await;
    let owner = make_user(&pool, "owner", false).await;

    // Both Saga books are scanned "fiction". A subjects override replaces
    // Book Two's tag list wholesale, so only Book One may still match — the
    // canonical link row must not shine through the override.
    let uuid = uuid_by_title(&pool, "Saga: Book Two").await;
    let overrides = omnibus_shared::MetadataOverrides {
        subjects: Some(vec!["romance".into()]),
        ..Default::default()
    };
    crate::upsert_metadata_overrides(&pool, &uuid, &overrides, false, owner)
        .await
        .unwrap();

    let shelf = create_shelf(
        &pool,
        owner,
        &smart_req("Fiction", MatchMode::Any, vec![tag_rule("fiction")]),
    )
    .await
    .unwrap();
    assert_eq!(shelf.book_count, 1);
}

/// Create a fileless book tagged via a subjects override — the only way a
/// physical-only book carries tags (fileless creation writes no tag links).
async fn make_tagged_fileless_book(
    pool: &sqlx::SqlitePool,
    title: &str,
    tag: &str,
    editor: i64,
) -> String {
    let uuid = crate::physical::create_fileless_book(
        pool,
        crate::physical::FilelessBook {
            title: title.into(),
            authors: vec!["Ada Lovelace".into()],
            isbn: None,
            pubdate: None,
            description: None,
            cover: None,
        },
    )
    .await
    .unwrap();
    let overrides = omnibus_shared::MetadataOverrides {
        subjects: Some(vec![tag.into()]),
        ..Default::default()
    };
    crate::upsert_metadata_overrides(pool, &uuid, &overrides, false, editor)
        .await
        .unwrap();
    uuid
}

#[tokio::test]
async fn smart_shelf_tag_rule_matches_physical_only_book_with_copy() {
    let _covers = crate::test_support::CoversTempDir::new("smart_physical_copy");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let owner = make_user(&pool, "owner", false).await;

    // A checked-in physical book has no `book_files` row; it must still land
    // on a smart shelf whose tag rule its override subjects match, mirroring
    // the landing visibility rule (file-backed OR physical copy).
    let uuid = make_tagged_fileless_book(&pool, "Paper Only", "fantasy", owner).await;
    crate::physical::add_physical_copy(&pool, &uuid, None, Some(owner), None)
        .await
        .unwrap();

    let shelf = create_shelf(
        &pool,
        owner,
        &smart_req("Fantasy", MatchMode::Any, vec![tag_rule("fantasy")]),
    )
    .await
    .unwrap();
    assert_eq!(shelf.book_count, 1);

    let page = shelf_page(&pool, &shelf, SortKey::Title, SortDir::Asc)
        .await
        .unwrap();
    assert_eq!(page.books.len(), 1);
    assert_eq!(page.books[0].title.as_deref(), Some("Paper Only"));
}

#[tokio::test]
async fn smart_shelf_hides_fileless_book_without_physical_copy() {
    let _covers = crate::test_support::CoversTempDir::new("smart_no_copy");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let owner = make_user(&pool, "owner", false).await;

    // No file and no copy — a removed-file ghost or pure wishlist row. The
    // visibility gate must keep hiding it even though the tag rule matches.
    make_tagged_fileless_book(&pool, "Someday Maybe", "fantasy", owner).await;

    let shelf = create_shelf(
        &pool,
        owner,
        &smart_req("Fantasy", MatchMode::Any, vec![tag_rule("fantasy")]),
    )
    .await
    .unwrap();
    assert_eq!(shelf.book_count, 0);
}

#[tokio::test]
async fn kobo_sync_excludes_physical_only_smart_members() {
    let (pool, _covers) = seed_discovery_fixture().await;
    let owner = make_user(&pool, "owner", false).await;

    let physical = make_tagged_fileless_book(&pool, "Paper Only", "fiction", owner).await;
    crate::physical::add_physical_copy(&pool, &physical, None, Some(owner), None)
        .await
        .unwrap();

    let shelf = create_shelf(
        &pool,
        owner,
        &smart_req("Fiction", MatchMode::Any, vec![tag_rule("fiction")]),
    )
    .await
    .unwrap();
    // The web read includes the physical book alongside the two file-backed
    // fixture books…
    assert_eq!(shelf.book_count, 3);

    update_shelf(
        &pool,
        shelf.id,
        &UpdateShelfRequest {
            sync_to_kobo: Some(true),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    // …but the Kobo entitlement union stays file-backed only: a device can't
    // download a book that has no file.
    let uuids = kobo_synced_book_uuids(&pool, owner).await.unwrap();
    assert_eq!(uuids.len(), 2);
    assert!(!uuids.contains(&physical));
}

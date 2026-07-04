use super::*;
use crate::pool::init_db;
use crate::test_support::{seed_discovery_fixture, seed_minimal_books};

use omnibus_shared::{
    CreateShelfRequest, MatchMode, RuleField, RuleOp, ShelfKind, ShelfRule, SortDir, SortKey,
    UpdateShelfRequest, Visibility,
};

async fn make_user(pool: &sqlx::SqlitePool, username: &str, is_admin: bool) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO users (username, password_hash, is_admin) VALUES (?, 'x', ?) RETURNING id",
    )
    .bind(username)
    .bind(is_admin)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn uuid_by_title(pool: &sqlx::SqlitePool, title: &str) -> String {
    sqlx::query_scalar::<_, String>("SELECT uuid FROM books WHERE title = ?")
        .bind(title)
        .fetch_one(pool)
        .await
        .unwrap()
}

fn tag_rule(value: &str) -> ShelfRule {
    ShelfRule {
        field: RuleField::Tag,
        op: RuleOp::Is,
        value: value.into(),
    }
}

fn smart_req(name: &str, mode: MatchMode, rules: Vec<ShelfRule>) -> CreateShelfRequest {
    CreateShelfRequest {
        kind: ShelfKind::Smart,
        name: name.into(),
        description: None,
        visibility: Visibility::Private,
        match_mode: Some(mode),
        rules,
        book_uuids: vec![],
    }
}

fn manual_req(name: &str, book_uuids: Vec<String>) -> CreateShelfRequest {
    CreateShelfRequest {
        kind: ShelfKind::Manual,
        name: name.into(),
        description: None,
        visibility: Visibility::Private,
        match_mode: None,
        rules: vec![],
        book_uuids,
    }
}

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
async fn smart_shelf_matches_author_by_name_case_insensitively() {
    let (pool, _covers) = seed_discovery_fixture().await;
    let owner = make_user(&pool, "owner", false).await;

    // "Ada Lovelace" authored 3 of the 4 fixture books. A lowercase value must
    // still match — regression: `author is` used to demand a numeric id, so a
    // typed name (any case) matched nothing.
    let rule = ShelfRule {
        field: RuleField::Author,
        op: RuleOp::Is,
        value: "ada lovelace".into(),
    };
    let shelf = create_shelf(
        &pool,
        owner,
        &smart_req("By Ada", MatchMode::Any, vec![rule]),
    )
    .await
    .unwrap();
    assert_eq!(shelf.book_count, 3);
}

#[tokio::test]
async fn smart_shelf_matches_series_starts_with() {
    let (pool, _covers) = seed_discovery_fixture().await;
    let owner = make_user(&pool, "owner", false).await;

    // Series "Saga" holds two books; a `starts with` prefix matches both.
    let rule = ShelfRule {
        field: RuleField::Series,
        op: RuleOp::StartsWith,
        value: "Sag".into(),
    };
    let shelf = create_shelf(
        &pool,
        owner,
        &smart_req("Saga-ish", MatchMode::Any, vec![rule]),
    )
    .await
    .unwrap();
    assert_eq!(shelf.book_count, 2);
}

#[tokio::test]
async fn smart_shelf_updates_when_a_qualifying_book_appears() {
    let (pool, _covers) = seed_discovery_fixture().await;
    let owner = make_user(&pool, "owner", false).await;
    let shelf = create_shelf(
        &pool,
        owner,
        &smart_req("Essays", MatchMode::Any, vec![tag_rule("essay")]),
    )
    .await
    .unwrap();
    assert_eq!(shelf.book_count, 1);

    // Tag another existing book "essay"; membership recomputes on next read.
    let book = sqlx::query_scalar::<_, i64>("SELECT id FROM books WHERE title = 'Standalone'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let _ = book; // Standalone already has "essay"; tag a second book instead.
    let other = sqlx::query_scalar::<_, i64>("SELECT id FROM books WHERE title = 'Other Story'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let tag_id: i64 = sqlx::query_scalar("SELECT id FROM tags WHERE name = 'essay'")
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO books_tags_link (book, tag) VALUES (?, ?)")
        .bind(other)
        .bind(tag_id)
        .execute(&pool)
        .await
        .unwrap();

    let reloaded = get_shelf(&pool, shelf.id).await.unwrap().unwrap();
    assert_eq!(reloaded.book_count, 2);
}

#[tokio::test]
async fn rating_rule_resolves_against_shelf_owner() {
    let (pool, _covers) = seed_discovery_fixture().await;
    let owner = make_user(&pool, "owner", false).await;
    let other = make_user(&pool, "other", false).await;
    let saga = uuid_by_title(&pool, "Saga: Book One").await;

    // Owner rates it 5★; the other user rates a different book 5★.
    sqlx::query("INSERT INTO user_ratings (user_id, book_uuid, half_stars) VALUES (?, ?, 10)")
        .bind(owner)
        .bind(&saga)
        .execute(&pool)
        .await
        .unwrap();
    let standalone = uuid_by_title(&pool, "Standalone").await;
    sqlx::query("INSERT INTO user_ratings (user_id, book_uuid, half_stars) VALUES (?, ?, 10)")
        .bind(other)
        .bind(&standalone)
        .execute(&pool)
        .await
        .unwrap();

    let rule = ShelfRule {
        field: RuleField::Rating,
        op: RuleOp::Gte,
        value: "4".into(),
    };
    let shelf = create_shelf(
        &pool,
        owner,
        &smart_req("Top rated", MatchMode::Any, vec![rule]),
    )
    .await
    .unwrap();
    // Only the owner's 5★ book qualifies — the other user's rating is invisible.
    assert_eq!(shelf.book_count, 1);
    let page = shelf_page(&pool, &shelf, SortKey::Title, SortDir::Asc)
        .await
        .unwrap();
    assert_eq!(page.books[0].title.as_deref(), Some("Saga: Book One"));
}

#[tokio::test]
async fn create_manual_shelf_keeps_added_order() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 3).await;
    let owner = make_user(&pool, "owner", false).await;

    let shelf = create_shelf(
        &pool,
        owner,
        &manual_req("Picks", vec!["uuid-3".into(), "uuid-1".into()]),
    )
    .await
    .unwrap();
    assert_eq!(shelf.book_count, 2);

    let page = shelf_page(&pool, &shelf, SortKey::Title, SortDir::Asc)
        .await
        .unwrap();
    let titles: Vec<_> = page.books.iter().filter_map(|b| b.title.clone()).collect();
    assert_eq!(titles, vec!["Title 3", "Title 1"]); // position order, not sort
}

#[tokio::test]
async fn create_manual_shelf_rejects_unknown_book_before_inserting() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let owner = make_user(&pool, "owner", false).await;

    let err = create_shelf(&pool, owner, &manual_req("Bad", vec!["nope".into()]))
        .await
        .unwrap_err();
    assert!(matches!(err, ShelfError::BookNotFound));
    // The shelf row must not have been created.
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM shelves")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
async fn add_and_remove_book_updates_membership() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 3).await;
    let owner = make_user(&pool, "owner", false).await;
    let shelf = create_shelf(&pool, owner, &manual_req("Picks", vec![]))
        .await
        .unwrap();

    add_books(&pool, shelf.id, &["uuid-1".into(), "uuid-2".into()], owner)
        .await
        .unwrap();
    assert_eq!(
        get_shelf(&pool, shelf.id)
            .await
            .unwrap()
            .unwrap()
            .book_count,
        2
    );

    remove_book(&pool, shelf.id, "uuid-1").await.unwrap();
    assert_eq!(
        get_shelf(&pool, shelf.id)
            .await
            .unwrap()
            .unwrap()
            .book_count,
        1
    );
}

#[tokio::test]
async fn manual_membership_survives_a_ghosted_file() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let owner = make_user(&pool, "owner", false).await;
    let shelf = create_shelf(&pool, owner, &manual_req("Picks", vec!["uuid-1".into()]))
        .await
        .unwrap();

    // Ghost the book: drop its file row, keep the `books` row (uuid preserved).
    sqlx::query(
        "DELETE FROM book_files WHERE book_id = (SELECT id FROM books WHERE uuid = 'uuid-1')",
    )
    .execute(&pool)
    .await
    .unwrap();

    let reloaded = get_shelf(&pool, shelf.id).await.unwrap().unwrap();
    assert_eq!(reloaded.book_count, 1, "ghosted book stays on the shelf");
    let page = shelf_page(&pool, &reloaded, SortKey::Title, SortDir::Asc)
        .await
        .unwrap();
    assert_eq!(page.books.len(), 1);
}

#[tokio::test]
async fn list_visible_scopes_by_owner_public_and_admin() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let alice = make_user(&pool, "alice", false).await;
    let bob = make_user(&pool, "bob", false).await;
    let admin = make_user(&pool, "admin", true).await;

    create_shelf(&pool, alice, &manual_req("Alice private", vec![]))
        .await
        .unwrap();
    let mut public = manual_req("Alice public", vec![]);
    public.visibility = Visibility::Public;
    create_shelf(&pool, alice, &public).await.unwrap();

    // Bob sees only Alice's public shelf.
    let bob_view = list_visible_shelves(&pool, bob, false).await.unwrap();
    assert_eq!(bob_view.len(), 1);
    assert_eq!(bob_view[0].name, "Alice public");

    // Alice sees both of her own.
    assert_eq!(
        list_visible_shelves(&pool, alice, false)
            .await
            .unwrap()
            .len(),
        2
    );
    // Admin sees everything.
    assert_eq!(
        list_visible_shelves(&pool, admin, true)
            .await
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn duplicate_name_for_same_owner_is_name_taken() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let owner = make_user(&pool, "owner", false).await;
    create_shelf(&pool, owner, &manual_req("Favorites", vec![]))
        .await
        .unwrap();

    let err = create_shelf(&pool, owner, &manual_req("favorites", vec![]))
        .await
        .unwrap_err();
    assert!(matches!(err, ShelfError::NameTaken));
}

#[tokio::test]
async fn same_name_for_different_owners_is_allowed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let alice = make_user(&pool, "alice", false).await;
    let bob = make_user(&pool, "bob", false).await;
    create_shelf(&pool, alice, &manual_req("Favorites", vec![]))
        .await
        .unwrap();
    // Different owner, same name — no conflict.
    create_shelf(&pool, bob, &manual_req("Favorites", vec![]))
        .await
        .unwrap();
}

#[tokio::test]
async fn update_shelf_changes_name_and_visibility() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let owner = make_user(&pool, "owner", false).await;
    let shelf = create_shelf(&pool, owner, &manual_req("Old", vec![]))
        .await
        .unwrap();

    let updated = update_shelf(
        &pool,
        shelf.id,
        &UpdateShelfRequest {
            name: Some("New".into()),
            visibility: Some(Visibility::Public),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(updated.name, "New");
    assert_eq!(updated.visibility, Visibility::Public);
}

#[tokio::test]
async fn update_manual_shelf_rejects_match_mode_and_rules() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let owner = make_user(&pool, "owner", false).await;
    let shelf = create_shelf(&pool, owner, &manual_req("Picks", vec![]))
        .await
        .unwrap();

    let with_mode = UpdateShelfRequest {
        match_mode: Some(MatchMode::All),
        ..Default::default()
    };
    assert!(matches!(
        update_shelf(&pool, shelf.id, &with_mode).await.unwrap_err(),
        ShelfError::InvalidRule(_)
    ));

    let with_rules = UpdateShelfRequest {
        rules: Some(vec![tag_rule("Fantasy")]),
        ..Default::default()
    };
    assert!(matches!(
        update_shelf(&pool, shelf.id, &with_rules)
            .await
            .unwrap_err(),
        ShelfError::InvalidRule(_)
    ));
    // The manual shelf stays rule-less.
    assert!(get_shelf(&pool, shelf.id)
        .await
        .unwrap()
        .unwrap()
        .match_mode
        .is_none());
}

#[tokio::test]
async fn update_smart_shelf_rejects_emptying_rules() {
    let (pool, _covers) = seed_discovery_fixture().await;
    let owner = make_user(&pool, "owner", false).await;
    let shelf = create_shelf(
        &pool,
        owner,
        &smart_req("Fiction", MatchMode::Any, vec![tag_rule("fiction")]),
    )
    .await
    .unwrap();

    let empty = UpdateShelfRequest {
        rules: Some(vec![]),
        ..Default::default()
    };
    assert!(matches!(
        update_shelf(&pool, shelf.id, &empty).await.unwrap_err(),
        ShelfError::InvalidRule(_)
    ));
}

#[tokio::test]
async fn update_and_delete_missing_shelf_return_not_found() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    assert!(matches!(
        update_shelf(&pool, 999, &UpdateShelfRequest::default())
            .await
            .unwrap_err(),
        ShelfError::NotFound
    ));
    assert!(matches!(
        delete_shelf(&pool, 999).await.unwrap_err(),
        ShelfError::NotFound
    ));
}

#[tokio::test]
async fn delete_shelf_cascades_rules_and_membership() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let owner = make_user(&pool, "owner", false).await;
    let shelf = create_shelf(&pool, owner, &manual_req("Picks", vec!["uuid-1".into()]))
        .await
        .unwrap();

    delete_shelf(&pool, shelf.id).await.unwrap();
    assert!(get_shelf(&pool, shelf.id).await.unwrap().is_none());
    let members: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM shelf_books WHERE shelf_id = ?")
        .bind(shelf.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(members, 0);
}

#[tokio::test]
async fn preview_rule_reports_matched_and_total() {
    let (pool, _covers) = seed_discovery_fixture().await;
    let owner = make_user(&pool, "owner", false).await;
    let preview = preview_rule(&pool, owner, MatchMode::Any, &[tag_rule("fiction")])
        .await
        .unwrap();
    assert_eq!(preview.matched, 2);
    assert_eq!(preview.total, 4);
    assert_eq!(preview.sample.len(), 2);
}

#[tokio::test]
async fn can_view_and_can_edit_enforce_visibility() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let owner = make_user(&pool, "owner", false).await;
    let mut req = manual_req("Private", vec![]);
    req.visibility = Visibility::Private;
    let shelf = create_shelf(&pool, owner, &req).await.unwrap();
    let full = get_shelf(&pool, shelf.id).await.unwrap().unwrap();

    assert!(can_view(&full, owner, false));
    assert!(!can_view(&full, owner + 1, false));
    assert!(can_view(&full, owner + 1, true)); // admin
    assert!(can_edit(&full, owner, false));
    assert!(!can_edit(&full, owner + 1, false));
    assert!(can_edit(&full, owner + 1, true)); // admin
}

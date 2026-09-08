//! Create/update/delete: name uniqueness per owner, the update
//! validations (manual shelves take no rules, smart shelves keep at least
//! one), rule replacement changing membership and rolling back on a rename
//! collision, cascade on delete, the view/edit permission checks, and the
//! DB-failure paths.

use omnibus_shared::{
    MatchMode, RuleField, RuleOp, ShelfRule, SortDir, SortKey, UpdateShelfRequest, Visibility,
};

use super::super::*;
use super::{make_user, manual_req, smart_req, tag_rule};
use crate::pool::init_db;
use crate::test_support::{seed_discovery_fixture, seed_minimal_books};

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
async fn update_shelf_rules_changes_membership_of_an_existing_smart_shelf() {
    let (pool, _covers) = seed_discovery_fixture().await;
    let owner = make_user(&pool, "owner", false).await;
    let shelf = create_shelf(
        &pool,
        owner,
        &smart_req("Rotating", MatchMode::Any, vec![tag_rule("fiction")]),
    )
    .await
    .unwrap();
    assert_eq!(shelf.book_count, 2, "starts matching the 'fiction' tag");

    // Retarget the same shelf at a different tag with `all` matching plus a
    // second condition — an unrelated field so the two together still narrow
    // to the single "essay"-tagged book.
    let updated = update_shelf(
        &pool,
        shelf.id,
        &UpdateShelfRequest {
            match_mode: Some(MatchMode::All),
            rules: Some(vec![
                tag_rule("essay"),
                ShelfRule {
                    field: RuleField::Author,
                    op: RuleOp::Is,
                    value: "ada lovelace".into(),
                },
            ]),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(updated.match_mode, Some(MatchMode::All));
    assert_eq!(
        updated.book_count, 1,
        "membership must recompute from the new rules, not the old 'fiction' set"
    );

    let page = shelf_page(&pool, &updated, SortKey::Title, SortDir::Asc)
        .await
        .unwrap();
    assert_eq!(page.books.len(), 1);
    assert_eq!(page.books[0].title.as_deref(), Some("Standalone"));
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
async fn update_shelf_rolls_back_rule_replacement_when_rename_collides() {
    // The rule replacement and the shelf-row rename share one transaction:
    // if the rename fails (name already taken), the just-inserted new rules
    // must roll back too, leaving the shelf's original rules intact.
    let (pool, _covers) = seed_discovery_fixture().await;
    let owner = make_user(&pool, "owner", false).await;
    create_shelf(&pool, owner, &manual_req("Existing", vec![]))
        .await
        .unwrap();
    let target = create_shelf(
        &pool,
        owner,
        &smart_req("Target", MatchMode::Any, vec![tag_rule("fiction")]),
    )
    .await
    .unwrap();

    let err = update_shelf(
        &pool,
        target.id,
        &UpdateShelfRequest {
            name: Some("Existing".into()),
            rules: Some(vec![tag_rule("mystery")]),
            ..Default::default()
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(err, ShelfError::NameTaken));

    let reloaded = get_shelf(&pool, target.id).await.unwrap().unwrap();
    assert_eq!(reloaded.name, "Target", "the rename did not apply");
    assert_eq!(
        reloaded.rules,
        vec![tag_rule("fiction")],
        "the rule replacement was rolled back along with the failed rename"
    );
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

#[tokio::test]
async fn public_shelf_is_viewable_but_not_editable_by_non_owner() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let owner = make_user(&pool, "owner", false).await;
    let mut req = manual_req("Public", vec![]);
    req.visibility = Visibility::Public;
    let shelf = create_shelf(&pool, owner, &req).await.unwrap();
    let full = get_shelf(&pool, shelf.id).await.unwrap().unwrap();

    // AC1: a non-owner may read a public shelf. AC2: but never mutate it —
    // the guard the rpc layer (`shelf_for_edit`) enforces before every write.
    assert!(can_view(&full, owner + 1, false));
    assert!(!can_edit(&full, owner + 1, false));
}

#[tokio::test]
async fn get_shelf_propagates_db_error_when_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    pool.close().await;
    let err = get_shelf(&pool, 1).await.unwrap_err();
    assert!(matches!(err, ShelfError::Sqlx(_)));
}

#[tokio::test]
async fn create_shelf_propagates_db_error_when_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    pool.close().await;
    let err = create_shelf(&pool, 1, &manual_req("Closed", vec![]))
        .await
        .unwrap_err();
    assert!(matches!(err, ShelfError::Sqlx(_)));
}

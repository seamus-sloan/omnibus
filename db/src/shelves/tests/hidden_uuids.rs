//! `shelf_exclusive_hidden_uuids`: a book confined to a private shelf is
//! hidden from a non-owner, stays visible on a public shelf, with no
//! membership, to an admin, or when any one of several shelves is
//! visible; plus the DB-failure path.

use omnibus_shared::Visibility;

use super::super::*;
use super::{make_user, manual_req};
use crate::pool::init_db;
use crate::test_support::seed_minimal_books;

#[tokio::test]
async fn shelf_exclusive_hidden_uuids_hides_a_book_confined_to_a_private_shelf_from_a_non_owner() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let alice = make_user(&pool, "alice", false).await;
    let bob = make_user(&pool, "bob", false).await;
    create_shelf(
        &pool,
        alice,
        &manual_req("Secret Stack", vec!["uuid-1".into()]),
    )
    .await
    .unwrap();

    let hidden = shelf_exclusive_hidden_uuids(&pool, bob, false, &["uuid-1".to_string()])
        .await
        .unwrap();
    assert!(hidden.contains("uuid-1"), "non-owner must not see it");

    let hidden = shelf_exclusive_hidden_uuids(&pool, alice, false, &["uuid-1".to_string()])
        .await
        .unwrap();
    assert!(hidden.is_empty(), "the owner must still see their own book");
}

#[tokio::test]
async fn shelf_exclusive_hidden_uuids_does_not_hide_a_book_on_a_public_shelf() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let alice = make_user(&pool, "alice", false).await;
    let bob = make_user(&pool, "bob", false).await;
    let mut req = manual_req("Front Window", vec!["uuid-1".into()]);
    req.visibility = Visibility::Public;
    create_shelf(&pool, alice, &req).await.unwrap();

    let hidden = shelf_exclusive_hidden_uuids(&pool, bob, false, &["uuid-1".to_string()])
        .await
        .unwrap();
    assert!(hidden.is_empty(), "a public shelf's books stay visible");
}

#[tokio::test]
async fn shelf_exclusive_hidden_uuids_does_not_hide_a_book_with_no_shelf_membership() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 2).await;
    let alice = make_user(&pool, "alice", false).await;
    let bob = make_user(&pool, "bob", false).await;
    create_shelf(
        &pool,
        alice,
        &manual_req("Secret Stack", vec!["uuid-1".into()]),
    )
    .await
    .unwrap();

    // uuid-2 is on no shelf at all — never affected regardless of viewer.
    let hidden = shelf_exclusive_hidden_uuids(&pool, bob, false, &["uuid-2".to_string()])
        .await
        .unwrap();
    assert!(hidden.is_empty());
}

#[tokio::test]
async fn shelf_exclusive_hidden_uuids_treats_an_admin_as_able_to_see_every_shelf() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let alice = make_user(&pool, "alice", false).await;
    let admin = make_user(&pool, "admin", true).await;
    create_shelf(
        &pool,
        alice,
        &manual_req("Secret Stack", vec!["uuid-1".into()]),
    )
    .await
    .unwrap();

    let hidden = shelf_exclusive_hidden_uuids(&pool, admin, true, &["uuid-1".to_string()])
        .await
        .unwrap();
    assert!(hidden.is_empty());
}

#[tokio::test]
async fn shelf_exclusive_hidden_uuids_stays_visible_when_any_one_of_several_shelves_is_visible() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let alice = make_user(&pool, "alice", false).await;
    let bob = make_user(&pool, "bob", false).await;
    let carol = make_user(&pool, "carol", false).await;
    // Both private, owned by different users — the book is on two shelves
    // Carol can't see, but Bob's own private shelf still covers it.
    create_shelf(
        &pool,
        alice,
        &manual_req("Alice's private", vec!["uuid-1".into()]),
    )
    .await
    .unwrap();
    create_shelf(
        &pool,
        bob,
        &manual_req("Bob's private", vec!["uuid-1".into()]),
    )
    .await
    .unwrap();

    let hidden = shelf_exclusive_hidden_uuids(&pool, bob, false, &["uuid-1".to_string()])
        .await
        .unwrap();
    assert!(hidden.is_empty(), "Bob owns one of the two shelves");

    let hidden = shelf_exclusive_hidden_uuids(&pool, carol, false, &["uuid-1".to_string()])
        .await
        .unwrap();
    assert!(
        hidden.contains("uuid-1"),
        "Carol can see neither shelf the book is confined to"
    );
}

#[tokio::test]
async fn shelf_exclusive_hidden_uuids_propagates_db_error_when_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    pool.close().await;
    let err = shelf_exclusive_hidden_uuids(&pool, 1, false, &["uuid-1".to_string()])
        .await
        .unwrap_err();
    assert!(matches!(err, ShelfError::Sqlx(_)));
}

//! Unit tests for shelves, split by sub-topic into the sibling modules
//! below; the user, request-builder and lookup fixtures they share live
//! here. Covers smart-shelf rule matching, manual membership, listing and
//! visibility, create/update/delete, the built-in Wishlist shelf, and the
//! shelf-exclusive hidden set.

mod crud;
mod hidden_uuids;
mod listing;
mod membership;
mod smart_fields;
mod smart_rules;
mod wishlist;

use omnibus_shared::{
    CreateShelfRequest, MatchMode, RuleField, RuleOp, ShelfKind, ShelfRule, Visibility,
};

use super::*;

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

/// The owner's wishlist shelf id, provisioning it first (mirrors what
/// `create_user` / the boot backfill do — `make_user` inserts raw).
async fn wishlist_shelf_id(pool: &sqlx::SqlitePool, owner: i64) -> i64 {
    provision_wishlist_shelf(pool, owner).await.unwrap();
    sqlx::query_scalar::<_, i64>(
        "SELECT id FROM shelves WHERE owner_user_id = ? AND kind = 'wishlist'",
    )
    .bind(owner)
    .fetch_one(pool)
    .await
    .unwrap()
}

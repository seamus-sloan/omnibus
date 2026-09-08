//! `list_visible_shelves` and the shelf page: owner/public/admin scoping,
//! the viewer's own shelves first, the hard cap, batched per-shelf counts,
//! the mosaic cover uuids, owner attribution by display name, and the
//! recently-interacted ordering.

use omnibus_shared::{MatchMode, RuleField, RuleOp, ShelfRule, SortDir, SortKey, Visibility};

use super::super::*;
use super::{make_user, manual_req, smart_req, tag_rule, uuid_by_title, wishlist_shelf_id};
use crate::physical::add_wishlist_entry;
use crate::pool::init_db;
use crate::test_support::{seed_discovery_fixture, seed_minimal_books};
use omnibus_shared::physical::WishlistSource;

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

    // Bob sees only Alice's public shelf, attributed to its owner.
    let bob_view = list_visible_shelves(&pool, bob, false).await.unwrap();
    assert_eq!(bob_view.len(), 1);
    assert_eq!(bob_view[0].name, "Alice public");
    assert_eq!(bob_view[0].owner_username, "alice");

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
async fn list_visible_shelves_orders_the_viewers_own_shelves_before_other_owners() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let alice = make_user(&pool, "alice", false).await;
    let bob = make_user(&pool, "bob", false).await;

    // Alice's public shelf is created first, so it wins on `position, id`
    // alone — only the owner term can pull Bob's own shelves ahead of it.
    let mut alice_public = manual_req("Alice public", vec![]);
    alice_public.visibility = Visibility::Public;
    create_shelf(&pool, alice, &alice_public).await.unwrap();
    create_shelf(&pool, bob, &manual_req("Bob first", vec![]))
        .await
        .unwrap();
    create_shelf(&pool, bob, &manual_req("Bob second", vec![]))
        .await
        .unwrap();

    let names: Vec<String> = list_visible_shelves(&pool, bob, false)
        .await
        .unwrap()
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert_eq!(names, ["Bob first", "Bob second", "Alice public"]);

    // Alice sees her own first for the same reason, and `position, id` still
    // orders within each group.
    let names: Vec<String> = list_visible_shelves(&pool, alice, false)
        .await
        .unwrap()
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert_eq!(names, ["Alice public"]);
}

#[tokio::test]
async fn list_visible_shelves_orders_an_admins_own_shelves_first_across_the_whole_instance() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let alice = make_user(&pool, "alice", false).await;
    let admin = make_user(&pool, "admin", true).await;

    create_shelf(&pool, alice, &manual_req("Alice private", vec![]))
        .await
        .unwrap();
    create_shelf(&pool, admin, &manual_req("Admin shelf", vec![]))
        .await
        .unwrap();

    // The admin read is unscoped, so it is the one place where a stranger's
    // *private* shelf could outrank the viewer's own without this ordering.
    let names: Vec<String> = list_visible_shelves(&pool, admin, true)
        .await
        .unwrap()
        .into_iter()
        .map(|s| s.name)
        .collect();
    assert_eq!(names, ["Admin shelf", "Alice private"]);
}

#[tokio::test]
async fn get_shelf_carries_owner_username() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let alice = make_user(&pool, "alice", false).await;
    let shelf = create_shelf(&pool, alice, &manual_req("Alice shelf", vec![]))
        .await
        .unwrap();

    let fetched = get_shelf(&pool, shelf.id).await.unwrap().unwrap();
    assert_eq!(fetched.owner_username, "alice");
}

/// Bulk-insert `count` manual shelf rows owned by `owner_id` without going
/// through `create_shelf` — too slow at over-cap row counts.
async fn seed_shelves_raw(pool: &sqlx::SqlitePool, owner_id: i64, count: i64) {
    sqlx::query(
        r"
        WITH RECURSIVE n(i) AS (
            SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < ?
        )
        INSERT INTO shelves (owner_user_id, kind, name, position)
        SELECT ?, 'manual', 'Shelf ' || i, i FROM n
        ",
    )
    .bind(count)
    .bind(owner_id)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn list_visible_shelves_caps_response_at_hard_limit() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let owner = make_user(&pool, "owner", false).await;
    let over_cap = LIST_SHELVES_LIMIT + 50;
    seed_shelves_raw(&pool, owner, over_cap).await;

    let list = list_visible_shelves(&pool, owner, false).await.unwrap();
    assert_eq!(
        list.len() as i64,
        LIST_SHELVES_LIMIT,
        "list_visible_shelves must not return more than LIST_SHELVES_LIMIT rows",
    );
}

#[tokio::test]
async fn list_visible_shelves_reports_correct_per_shelf_counts_when_batched() {
    // Exercises the batched rule-load / manual-count path with more than one
    // shelf of each kind, so a shelf_id mix-up in the batching would surface
    // as a wrong count on the wrong shelf.
    let (pool, _covers) = seed_discovery_fixture().await;
    let owner = make_user(&pool, "owner", false).await;

    let fiction = create_shelf(
        &pool,
        owner,
        &smart_req("Fiction", MatchMode::Any, vec![tag_rule("fiction")]),
    )
    .await
    .unwrap();
    let empty_smart = create_shelf(
        &pool,
        owner,
        &smart_req("No matches", MatchMode::Any, vec![tag_rule("no-such-tag")]),
    )
    .await
    .unwrap();

    let book_a = uuid_by_title(&pool, "Saga: Book One").await;
    let manual_with_book =
        create_shelf(&pool, owner, &manual_req("Manual with book", vec![book_a]))
            .await
            .unwrap();
    let manual_empty = create_shelf(&pool, owner, &manual_req("Manual empty", vec![]))
        .await
        .unwrap();

    let shelves = list_visible_shelves(&pool, owner, false).await.unwrap();
    let count_for = |id: i64| shelves.iter().find(|s| s.id == id).unwrap().book_count;

    assert_eq!(count_for(fiction.id), 2, "two books tagged fiction");
    assert_eq!(count_for(empty_smart.id), 0, "no book matches the tag");
    assert_eq!(count_for(manual_with_book.id), 1);
    assert_eq!(
        count_for(manual_empty.id),
        0,
        "an empty manual shelf has no shelf_books row and must default to 0"
    );
}

#[tokio::test]
async fn list_visible_shelves_computes_smart_counts_for_a_mix_of_owner_and_public_shelves() {
    // Regression for the N+1: three smart shelves (one the viewer's own, two
    // owned by other users but public) must each get their own correct
    // fanned-out count rather than a mixed-up or dropped one.
    let (pool, _covers) = seed_discovery_fixture().await;
    let viewer = make_user(&pool, "viewer", false).await;
    let other_a = make_user(&pool, "other_a", false).await;
    let other_b = make_user(&pool, "other_b", false).await;

    let mine = create_shelf(
        &pool,
        viewer,
        &smart_req("Mine", MatchMode::Any, vec![tag_rule("fiction")]),
    )
    .await
    .unwrap();

    let mut other_a_req = smart_req(
        "Other A public",
        MatchMode::Any,
        vec![ShelfRule {
            field: RuleField::Author,
            op: RuleOp::Is,
            value: "ada lovelace".into(),
        }],
    );
    other_a_req.visibility = Visibility::Public;
    let other_a_shelf = create_shelf(&pool, other_a, &other_a_req).await.unwrap();

    let mut other_b_req = smart_req(
        "Other B public",
        MatchMode::Any,
        vec![ShelfRule {
            field: RuleField::Series,
            op: RuleOp::StartsWith,
            value: "Sag".into(),
        }],
    );
    other_b_req.visibility = Visibility::Public;
    let other_b_shelf = create_shelf(&pool, other_b, &other_b_req).await.unwrap();

    let shelves = list_visible_shelves(&pool, viewer, false).await.unwrap();
    let ids: std::collections::HashSet<i64> = shelves.iter().map(|s| s.id).collect();
    for (id, label) in [
        (mine.id, "the viewer's own smart shelf"),
        (other_a_shelf.id, "other_a's public smart shelf"),
        (other_b_shelf.id, "other_b's public smart shelf"),
    ] {
        assert!(ids.contains(&id), "{label} must be in the visible set");
    }
    let count_for = |id: i64| shelves.iter().find(|s| s.id == id).unwrap().book_count;

    assert_eq!(count_for(mine.id), 2, "two books tagged fiction");
    assert_eq!(
        count_for(other_a_shelf.id),
        3,
        "three books authored by Ada Lovelace"
    );
    assert_eq!(
        count_for(other_b_shelf.id),
        2,
        "two books in the Saga series"
    );
}

// Mosaic cover uuids on ShelfSummary (landing shelf gallery)
#[tokio::test]
async fn list_visible_shelves_caps_manual_cover_uuids_at_four_cover_bearing_members() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let owner = make_user(&pool, "owner", false).await;
    seed_minimal_books(&pool, 6).await;
    // First member lacks a cover: the mosaic must skip it, then cap at four.
    sqlx::query("UPDATE books SET has_cover = 1 WHERE uuid != 'uuid-1'")
        .execute(&pool)
        .await
        .unwrap();

    let members: Vec<String> = (1..=6).map(|i| format!("uuid-{i}")).collect();
    let shelf = create_shelf(&pool, owner, &manual_req("Six", members))
        .await
        .unwrap();

    let shelves = list_visible_shelves(&pool, owner, false).await.unwrap();
    let summary = shelves.iter().find(|s| s.id == shelf.id).unwrap();
    assert_eq!(
        summary.cover_uuids,
        vec!["uuid-2", "uuid-3", "uuid-4", "uuid-5"],
        "mosaic skips coverless members and caps at four, in shelf position order"
    );
}

#[tokio::test]
async fn list_visible_shelves_returns_smart_and_wishlist_cover_uuids() {
    let (pool, _covers) = seed_discovery_fixture().await;
    let owner = make_user(&pool, "owner", false).await;
    // Every fixture book except Saga Two gets a cover; the smart mosaic must
    // exclude the uncovered match.
    sqlx::query("UPDATE books SET has_cover = 1 WHERE title != 'Saga: Book Two'")
        .execute(&pool)
        .await
        .unwrap();

    let fiction = create_shelf(
        &pool,
        owner,
        &smart_req("Fiction", MatchMode::Any, vec![tag_rule("fiction")]),
    )
    .await
    .unwrap();

    let saga1 = uuid_by_title(&pool, "Saga: Book One").await;
    let standalone = uuid_by_title(&pool, "Standalone").await;
    let wishlist = wishlist_shelf_id(&pool, owner).await;
    add_wishlist_entry(&pool, owner, &saga1, WishlistSource::Detail)
        .await
        .unwrap();
    add_wishlist_entry(&pool, owner, &standalone, WishlistSource::Detail)
        .await
        .unwrap();

    let shelves = list_visible_shelves(&pool, owner, false).await.unwrap();
    let covers_for = |id: i64| {
        shelves
            .iter()
            .find(|s| s.id == id)
            .unwrap()
            .cover_uuids
            .clone()
    };

    assert_eq!(
        covers_for(fiction.id),
        vec![saga1.clone()],
        "only the cover-bearing fiction match feeds the smart mosaic"
    );
    assert_eq!(
        covers_for(wishlist),
        vec![standalone, saga1],
        "wishlist mosaic is newest-entry first, matching fetch_wishlist order"
    );
}

#[tokio::test]
async fn list_visible_shelves_returns_empty_cover_uuids_for_empty_shelf() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let owner = make_user(&pool, "owner", false).await;
    let shelf = create_shelf(&pool, owner, &manual_req("Empty", vec![]))
        .await
        .unwrap();

    let shelves = list_visible_shelves(&pool, owner, false).await.unwrap();
    let summary = shelves.iter().find(|s| s.id == shelf.id).unwrap();
    assert!(summary.cover_uuids.is_empty());
}

#[tokio::test]
async fn shelf_owner_attribution_uses_the_display_name_when_one_is_set() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let owner = make_user(&pool, "cool-guy-7", false).await;
    let id = create_shelf(&pool, owner, &manual_req("Favourites", vec![]))
        .await
        .unwrap()
        .id;

    crate::auth::set_display_name(&pool, owner, Some("Seamus"))
        .await
        .unwrap();

    let shelf = get_shelf(&pool, id).await.unwrap().unwrap();
    assert_eq!(shelf.owner_username, "Seamus");
    let listed = list_visible_shelves(&pool, owner, false).await.unwrap();
    let row = listed.iter().find(|s| s.id == id).unwrap();
    assert_eq!(row.owner_username, "Seamus");
}

#[tokio::test]
async fn shelf_page_recently_interacted_orders_the_latest_signal_first() {
    let (pool, _covers) = seed_discovery_fixture().await;
    let owner = make_user(&pool, "owner", false).await;
    let shelf = create_shelf(
        &pool,
        owner,
        &smart_req("Fiction", MatchMode::Any, vec![tag_rule("fiction")]),
    )
    .await
    .unwrap();

    // Flatten the fixture's clocks so the rating below is unambiguously the
    // most recent thing that happened to either book.
    sqlx::query("UPDATE books SET timestamp = 1000, last_modified = 1000")
        .execute(&pool)
        .await
        .unwrap();
    let page = shelf_page(&pool, &shelf, SortKey::Title, SortDir::Asc)
        .await
        .unwrap();
    let last_by_title = page
        .books
        .last()
        .unwrap()
        .unique_identifier
        .clone()
        .unwrap();

    sqlx::query(
        "INSERT INTO user_ratings (user_id, book_uuid, half_stars, updated_at)
         VALUES (?, ?, 10, 9000)",
    )
    .bind(owner)
    .bind(&last_by_title)
    .execute(&pool)
    .await
    .unwrap();

    let page = shelf_page(&pool, &shelf, SortKey::RecentlyInteracted, SortDir::Desc)
        .await
        .unwrap();
    assert_eq!(
        page.books.first().unwrap().unique_identifier.as_deref(),
        Some(last_by_title.as_str()),
        "the freshly rated book must lead the shelf on this axis"
    );
}

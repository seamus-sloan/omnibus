//! `ShelfAccess::resolve` for the owner, an admin, a stranger, and a system
//! shelf; the refusal each lock carries; and the owner-attribution rule the
//! landing shelves row shares.

use omnibus_shared::{ShelfKind, UserSummary};

use super::*;

const OWNER: i64 = 1;

fn viewer(id: i64, is_admin: bool) -> UserSummary {
    UserSummary {
        id,
        username: format!("user{id}"),
        is_admin,
        can_upload: false,
        can_edit: false,
        can_download: true,
        kindle_email: None,
        display_name: None,
        has_avatar: false,
        hidden_formats: Vec::new(),
        book_detail_scroll_stops: false,
    }
}

fn resolve(viewer: Option<&UserSummary>, kind: ShelfKind) -> ShelfAccess {
    ShelfAccess::resolve(viewer, OWNER, "Alice", kind)
}

#[test]
fn resolve_is_pending_until_the_viewer_is_known() {
    let access = resolve(None, ShelfKind::Manual);
    assert_eq!(access, ShelfAccess::Pending);
    assert!(!access.can_edit());
    assert_eq!(access.lock_reason(), None);
}

#[test]
fn resolve_gives_the_owner_full_access() {
    let access = resolve(Some(&viewer(OWNER, false)), ShelfKind::Smart);
    assert_eq!(access, ShelfAccess::Owner);
    assert!(access.can_edit());
}

#[test]
fn resolve_lets_an_admin_change_someone_elses_shelf() {
    let access = resolve(Some(&viewer(9, true)), ShelfKind::Manual);
    assert_eq!(access, ShelfAccess::Admin);
    assert!(access.can_edit());
}

#[test]
fn resolve_locks_a_strangers_shelf_and_names_its_owner() {
    let access = resolve(Some(&viewer(9, false)), ShelfKind::Manual);
    assert!(!access.can_edit());
    let reason = access.lock_reason().expect("a stranger's shelf is locked");
    assert_eq!(
        reason.headline(),
        "You can\u{2019}t edit this shelf \u{2014} it belongs to Alice."
    );
    assert_eq!(
        reason.detail(),
        "Only its owner or an admin can rename it, change its books, or delete it."
    );
}

#[test]
fn resolve_locks_a_wishlist_even_for_its_owner_and_for_admins() {
    let own = resolve(Some(&viewer(OWNER, true)), ShelfKind::Wishlist);
    assert_eq!(
        own,
        ShelfAccess::Locked(LockReason::System {
            own: true,
            owner: "Alice".into()
        })
    );
    assert_eq!(
        own.lock_reason().map(LockReason::headline).as_deref(),
        Some("Your wishlist can\u{2019}t be edited here.")
    );

    let theirs = resolve(Some(&viewer(9, true)), ShelfKind::Wishlist);
    assert!(!theirs.can_edit());
    assert_eq!(
        theirs.lock_reason().map(LockReason::headline).as_deref(),
        Some("Alice\u{2019}s wishlist can\u{2019}t be edited.")
    );
}

#[test]
fn shows_owner_attribution_is_false_for_the_viewers_own_shelf() {
    assert!(!shows_owner_attribution(Some(1), 1, ShelfKind::Manual));
}

#[test]
fn shows_owner_attribution_is_true_for_a_shelf_the_viewer_does_not_own() {
    assert!(shows_owner_attribution(Some(1), 2, ShelfKind::Manual));
}

#[test]
fn shows_owner_attribution_is_false_for_a_wishlist_even_when_not_owned() {
    // A wishlist's name already opens with the owner, so the chip would repeat it.
    assert!(!shows_owner_attribution(Some(1), 2, ShelfKind::Wishlist));
}

#[test]
fn shows_owner_attribution_is_false_while_the_viewer_is_still_unknown() {
    // `viewer_id` is `None` until the boot effect resolves (SSR + first
    // paint); attribution must not show up before we know who's looking.
    assert!(!shows_owner_attribution(None, 2, ShelfKind::Manual));
}

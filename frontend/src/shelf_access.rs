//! Who may change a shelf, and — when the answer is no — why, in words the
//! greyed-out controls can show. Mirrors the server's rule (owner or admin,
//! never a system shelf) so the UI neither offers an edit the server would
//! refuse nor hides one without saying so.

use omnibus_shared::{ShelfKind, UserSummary};

/// What the viewer may do with one shelf.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShelfAccess {
    /// The viewer isn't resolved yet (SSR and first paint).
    Pending,
    /// The viewer owns the shelf.
    Owner,
    /// An admin changing someone else's shelf — allowed, but worth saying.
    Admin,
    /// The viewer may look but not change anything.
    Locked(LockReason),
}

/// Why a shelf is read-only for this viewer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LockReason {
    /// Someone else's shelf, and the viewer isn't an admin.
    NotOwner { owner: String },
    /// The built-in Wishlist, which fills itself. `own` when it's the viewer's.
    System { own: bool, owner: String },
}

impl ShelfAccess {
    /// Resolve access for `viewer` (`None` while unresolved) to a shelf with
    /// this owner and kind.
    pub fn resolve(
        viewer: Option<&UserSummary>,
        owner_user_id: i64,
        owner_name: &str,
        kind: ShelfKind,
    ) -> Self {
        let Some(viewer) = viewer else {
            return ShelfAccess::Pending;
        };
        let own = viewer.id == owner_user_id;
        if kind.is_system() {
            ShelfAccess::Locked(LockReason::System {
                own,
                owner: owner_name.to_string(),
            })
        } else if own {
            ShelfAccess::Owner
        } else if viewer.is_admin {
            ShelfAccess::Admin
        } else {
            ShelfAccess::Locked(LockReason::NotOwner {
                owner: owner_name.to_string(),
            })
        }
    }

    /// `true` when the edit controls should act.
    pub fn can_edit(&self) -> bool {
        matches!(self, ShelfAccess::Owner | ShelfAccess::Admin)
    }

    /// The reason to show beside the greyed-out controls, if they are.
    pub fn lock_reason(&self) -> Option<&LockReason> {
        match self {
            ShelfAccess::Locked(reason) => Some(reason),
            _ => None,
        }
    }
}

impl LockReason {
    /// The one-line refusal.
    pub fn headline(&self) -> String {
        match self {
            LockReason::NotOwner { owner } => {
                format!("You can\u{2019}t edit this shelf \u{2014} it belongs to {owner}.")
            }
            LockReason::System { own: true, .. } => {
                "Your wishlist can\u{2019}t be edited here.".into()
            }
            LockReason::System { own: false, owner } => {
                format!("{owner}\u{2019}s wishlist can\u{2019}t be edited.")
            }
        }
    }

    /// What would change it instead.
    pub fn detail(&self) -> String {
        match self {
            // The headline already names the owner; naming them again reads
            // badly when their name is itself "admin".
            LockReason::NotOwner { .. } => {
                "Only its owner or an admin can rename it, change its books, or delete it.".into()
            }
            LockReason::System { own: true, .. } => "It fills itself \u{2014} add a book to your \
                 wishlist from the book\u{2019}s page and it appears here."
                .into(),
            LockReason::System { own: false, .. } => {
                "A wishlist fills itself from what its owner wishes for.".into()
            }
        }
    }
}

/// `true` when a shelf entry should say whose it is: the viewer is known and
/// isn't the owner, and it isn't a Wishlist, whose name already opens with its
/// owner.
pub fn shows_owner_attribution(
    viewer_id: Option<i64>,
    owner_user_id: i64,
    kind: ShelfKind,
) -> bool {
    viewer_id.is_some_and(|vid| vid != owner_user_id) && kind != ShelfKind::Wishlist
}

#[cfg(test)]
mod tests;

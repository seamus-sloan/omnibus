//! Pure derivations behind the web shelves index: which shelves the filters
//! keep, how they group by owner, the order each group reads in, and the
//! header's census line. No rsx here, so every rule is unit-testable.

use std::cmp::Reverse;

use omnibus_shared::{IndexSort, ShelfKind, ShelfSummary, Visibility};

/// Whose shelves the index shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OwnerFilter {
    /// Every visible shelf.
    #[default]
    Anyone,
    /// Only the shelves this user id owns.
    Owner(i64),
}

/// Which shelf kinds the index shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum KindFilter {
    #[default]
    Any,
    Smart,
    Manual,
    Wishlist,
}

impl KindFilter {
    /// Every option, in toolbar order.
    pub const ALL: [KindFilter; 4] = [
        KindFilter::Any,
        KindFilter::Smart,
        KindFilter::Manual,
        KindFilter::Wishlist,
    ];

    /// `true` when a shelf of `kind` passes this filter.
    pub fn admits(self, kind: ShelfKind) -> bool {
        match self {
            KindFilter::Any => true,
            KindFilter::Smart => kind == ShelfKind::Smart,
            KindFilter::Manual => kind == ShelfKind::Manual,
            KindFilter::Wishlist => kind == ShelfKind::Wishlist,
        }
    }

    /// Toolbar label.
    pub fn label(self) -> &'static str {
        match self {
            KindFilter::Any => "All",
            KindFilter::Smart => "Smart",
            KindFilter::Manual => "Hand-picked",
            KindFilter::Wishlist => "Wishlists",
        }
    }

    /// Stable token for the toolbar's testids.
    pub fn token(self) -> &'static str {
        match self {
            KindFilter::Any => "any",
            KindFilter::Smart => "smart",
            KindFilter::Manual => "manual",
            KindFilter::Wishlist => "wishlist",
        }
    }
}

/// Everything that narrows or orders the index.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShelfQuery {
    pub text: String,
    pub owner: OwnerFilter,
    pub kind: KindFilter,
    pub sort: IndexSort,
}

impl ShelfQuery {
    /// `true` when anything hides shelves — the sort axis alone never does.
    pub fn is_filtering(&self) -> bool {
        !self.text.trim().is_empty()
            || self.owner != OwnerFilter::Anyone
            || self.kind != KindFilter::Any
    }
}

/// Someone with at least one visible shelf.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShelfOwner {
    pub id: i64,
    pub name: String,
    pub has_avatar: bool,
    /// `true` for the signed-in reader.
    pub is_viewer: bool,
    /// How many visible shelves they own, before any filter.
    pub shelf_count: usize,
}

/// One owner's shelves, in display order.
#[derive(Clone, Debug, PartialEq)]
pub struct OwnerGroup {
    pub owner: ShelfOwner,
    pub shelves: Vec<ShelfSummary>,
}

/// Every owner in `all`: the viewer first, then everyone else by name.
pub fn owners(all: &[ShelfSummary], viewer_id: Option<i64>) -> Vec<ShelfOwner> {
    let mut out: Vec<ShelfOwner> = Vec::new();
    for shelf in all {
        match out.iter_mut().find(|o| o.id == shelf.owner_user_id) {
            Some(owner) => owner.shelf_count += 1,
            None => out.push(ShelfOwner {
                id: shelf.owner_user_id,
                name: shelf.owner_username.clone(),
                has_avatar: shelf.owner_has_avatar,
                is_viewer: viewer_id == Some(shelf.owner_user_id),
                shelf_count: 1,
            }),
        }
    }
    out.sort_by_cached_key(|o| (!o.is_viewer, o.name.to_lowercase(), o.id));
    out
}

/// `true` when every whitespace-separated term in `text` appears in the
/// shelf's name or its owner's name, case-insensitively.
pub fn matches_text(shelf: &ShelfSummary, text: &str) -> bool {
    let haystack = format!("{} {}", shelf.name, shelf.owner_username).to_lowercase();
    text.split_whitespace()
        .all(|term| haystack.contains(&term.to_lowercase()))
}

/// Filter `all` by `query`, group the survivors by owner (viewer first), and
/// order each group by `query.sort`. An owner left with nothing drops out.
pub fn group_shelves(
    all: &[ShelfSummary],
    viewer_id: Option<i64>,
    query: &ShelfQuery,
) -> Vec<OwnerGroup> {
    owners(all, viewer_id)
        .into_iter()
        .filter(|owner| match query.owner {
            OwnerFilter::Anyone => true,
            OwnerFilter::Owner(id) => id == owner.id,
        })
        .filter_map(|owner| {
            let mut shelves: Vec<ShelfSummary> = all
                .iter()
                .filter(|s| {
                    s.owner_user_id == owner.id
                        && query.kind.admits(s.kind)
                        && matches_text(s, &query.text)
                })
                .cloned()
                .collect();
            if shelves.is_empty() {
                return None;
            }
            sort_group(&mut shelves, query.sort);
            Some(OwnerGroup { owner, shelves })
        })
        .collect()
}

/// Order one owner's shelves. The Wishlist trails under either axis: it is the
/// one shelf nobody curated, so it reads as the group's footer.
fn sort_group(shelves: &mut [ShelfSummary], sort: IndexSort) {
    match sort {
        IndexSort::Name => {
            shelves.sort_by_cached_key(|s| (s.kind.is_system(), s.name.to_lowercase(), s.id))
        }
        IndexSort::BookCount => shelves.sort_by_cached_key(|s| {
            (
                s.kind.is_system(),
                Reverse(s.book_count),
                s.name.to_lowercase(),
                s.id,
            )
        }),
    }
}

/// The header's census: how many shelves, how many are the viewer's, and how
/// many other readers the rest come from. A count line, not a sentence — no
/// trailing stop, and no flourish on top of the numbers.
pub fn census(all: &[ShelfSummary], viewer_id: Option<i64>) -> String {
    if all.is_empty() {
        return "No shelves yet".into();
    }
    let total = plural(all.len(), "shelf", "shelves");
    let yours = all
        .iter()
        .filter(|s| Some(s.owner_user_id) == viewer_id)
        .count();
    let others = all.len() - yours;
    let readers = owners(all, viewer_id)
        .iter()
        .filter(|o| !o.is_viewer)
        .count();
    match (yours, others) {
        // Every shelf is the viewer's, and the groups already say so.
        (_, 0) => total,
        (0, _) => format!("{total} from {}", plural(readers, "reader", "readers")),
        _ => format!(
            "{total} \u{b7} {yours} yours \u{b7} {others} from {}",
            plural(readers, "other reader", "other readers")
        ),
    }
}

/// `true` when the list holds someone else's private shelf — only an admin's
/// read can, and the header says so rather than let it pass as shared.
pub fn shows_others_private(all: &[ShelfSummary], viewer_id: Option<i64>) -> bool {
    all.iter()
        .any(|s| Some(s.owner_user_id) != viewer_id && s.visibility == Visibility::Private)
}

/// A group's heading.
pub fn group_title(owner: &ShelfOwner) -> String {
    if owner.is_viewer {
        "Your shelves".into()
    } else {
        format!("{}\u{2019}s shelves", owner.name)
    }
}

/// What the body says when no card survives: nothing to show at all, a text
/// filter that matched nothing, or toggles that did.
pub fn empty_message(query: &ShelfQuery, has_shelves: bool) -> String {
    let text = query.text.trim();
    if !has_shelves {
        "No shelves yet \u{2014} gather books by hand, or let a rule fill one for you.".into()
    } else if !text.is_empty() {
        format!("No shelves match \u{201c}{text}\u{201d}.")
    } else {
        "No shelves match these filters.".into()
    }
}

/// Kind word on a card.
pub fn kind_label(kind: ShelfKind) -> &'static str {
    match kind {
        ShelfKind::Smart => "Smart",
        ShelfKind::Manual => "Hand-picked",
        ShelfKind::Wishlist => "Wishlist",
    }
}

/// Visibility word on a card.
pub fn visibility_label(visibility: Visibility) -> &'static str {
    match visibility {
        Visibility::Private => "Private",
        Visibility::Public => "Public",
    }
}

/// `"1 book"` / `"N books"`.
pub fn book_count_label(count: i64) -> String {
    if count == 1 {
        "1 book".into()
    } else {
        format!("{count} books")
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

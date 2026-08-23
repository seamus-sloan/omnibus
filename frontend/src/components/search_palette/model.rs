//! Flat-list item model + small helpers shared between the overlay, the
//! keyboard handler, and the results list. The overlay builds a flat
//! `Vec<FlatItem>` from grouped `PaletteResults` so arrow-key navigation
//! has a single index space across Book / Author / Series / Tag / Genre rows.

use omnibus_shared::PaletteResults;

/// A single selectable item in the flat list used for arrow-key navigation.
/// Books carry the stable `uuid` so the palette can build `/books/:uuid`
/// URLs without a second round-trip — [`omnibus_shared::PaletteBookHit`]
/// now includes the uuid alongside `id`.
///
/// `Genre` is keyed on its name alone: genres have no row to navigate to, so
/// [`omnibus_shared::PaletteGenreHit`] carries no id (migration `0066`).
#[derive(Clone, Debug, PartialEq)]
pub(super) enum FlatItem {
    Book { uuid: String, title: String },
    Author { id: i64, name: String },
    Series { id: i64, name: String },
    Tag { id: i64, name: String },
    Genre { name: String },
}

/// Build a flat ordered list of all selectable items from the results.
pub(super) fn build_flat_items(results: &Option<PaletteResults>) -> Vec<FlatItem> {
    let Some(r) = results else {
        return Vec::new();
    };
    let mut items = Vec::new();
    for b in &r.books {
        items.push(FlatItem::Book {
            uuid: b.uuid.clone(),
            title: b.title.clone(),
        });
    }
    for a in &r.authors {
        items.push(FlatItem::Author {
            id: a.id,
            name: a.name.clone(),
        });
    }
    for s in &r.series {
        items.push(FlatItem::Series {
            id: s.id,
            name: s.name.clone(),
        });
    }
    for t in &r.tags {
        items.push(FlatItem::Tag {
            id: t.id,
            name: t.name.clone(),
        });
    }
    for g in &r.genres {
        items.push(FlatItem::Genre {
            name: g.name.clone(),
        });
    }
    items
}

/// Check if a given flat item matches the currently selected index.
pub(super) fn is_selected(items: &[FlatItem], selected_idx: usize, candidate: &FlatItem) -> bool {
    items.get(selected_idx) == Some(candidate)
}

/// Simple English plural suffix.
pub(super) fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// Build a facet query string where every whitespace-separated word in
/// `value` is prefixed with `prefix:`. This ensures `build_fts_match`
/// routes each token to the correct FTS5 column filter instead of
/// treating trailing words as free-text (e.g. `tag:Dark tag:academia`
/// rather than `tag:Dark academia`).
pub(super) fn facet_query(prefix: &str, value: &str) -> String {
    value
        .split_whitespace()
        .map(|w| format!("{prefix}:{w}"))
        .collect::<Vec<_>>()
        .join(" ")
}

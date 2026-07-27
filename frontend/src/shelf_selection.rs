//! Client-local persistence for the landing page's selected shelf, layered on
//! [`crate::client_store`] (localStorage on web, a JSON file on mobile, inert
//! on SSR). A single library-wide key — the gallery isn't scoped per library
//! path. Signals must still seed to [`ShelfSelection::All`] and reconcile in a
//! post-mount effect (rule 07): SSR and the first WASM paint both render All.

use serde::{Deserialize, Serialize};

/// Which lens the landing book list is showing: the whole library, or one
/// shelf overlaid on it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum ShelfSelection {
    #[default]
    All,
    Shelf(i64),
}

const STORAGE_KEY: &str = "omnibus.landing_shelf";

/// Load the persisted selection, or [`ShelfSelection::All`] when nothing is
/// stored or the JSON fails to parse.
pub fn load() -> ShelfSelection {
    crate::client_store::get(STORAGE_KEY)
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

/// Persist `sel`. Serialization/storage failures are silently ignored — the
/// UI keeps its in-memory copy regardless.
pub fn save(sel: ShelfSelection) {
    if let Ok(raw) = serde_json::to_string(&sel) {
        crate::client_store::set(STORAGE_KEY, &raw);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shelf_selection_round_trips_through_json() {
        for sel in [ShelfSelection::All, ShelfSelection::Shelf(42)] {
            let raw = serde_json::to_string(&sel).unwrap();
            assert_eq!(serde_json::from_str::<ShelfSelection>(&raw).unwrap(), sel);
        }
    }
}

#[cfg(all(test, not(any(feature = "web", feature = "mobile"))))]
mod ssr_tests {
    use super::*;

    #[test]
    fn load_returns_all_under_ssr() {
        // SSR `client_store` is inert, so load always yields All and save
        // can't change that — the hydration-parity contract the landing
        // gallery relies on.
        save(ShelfSelection::Shelf(7));
        assert_eq!(load(), ShelfSelection::All);
    }
}

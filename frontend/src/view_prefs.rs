//! Per-library view-preference persistence for [`ViewPrefs`], layered on the
//! durable [`crate::client_store`] (localStorage on web, a JSON file on mobile,
//! inert on SSR so first-hydration markup matches the WASM client). Keyed per
//! library path so each library keeps independent sort/filter state. Shape
//! lives in `omnibus-shared`.

use omnibus_shared::ViewPrefs;

const STORAGE_PREFIX: &str = "omnibus.view_prefs::";

/// Single library-wide pointer to the path [`save`] last wrote prefs for.
/// The landing page learns its own library path only from a page-1 *response*,
/// so without this pointer its first fetch is forced to go out on
/// [`ViewPrefs::default`] and the grid paints in the wrong order until a second
/// fetch corrects it (#1818). Reading the pointer on mount breaks that cycle.
const LAST_LIBRARY_KEY: &str = "omnibus.view_prefs.last_library";

fn storage_key(library_path: &str) -> String {
    format!("{STORAGE_PREFIX}{library_path}")
}

/// Load view preferences for `library_path`. Returns [`ViewPrefs::default`]
/// when no record exists, when storage is unavailable, or when the stored JSON
/// fails to parse.
pub fn load(library_path: &str) -> ViewPrefs {
    crate::client_store::get(&storage_key(library_path))
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

/// Persist `prefs` for `library_path`. Serialization or storage failures
/// (private mode, quota, an unwritable data dir) are silently ignored — the UI
/// keeps its in-memory copy regardless.
///
/// Also records `library_path` as the last-browsed library so [`load_last`] can
/// find these prefs before a fetch has revealed which library is on screen. The
/// pointer is only written once the prefs themselves serialized, so it can never
/// point at a key that was never stored.
pub fn save(library_path: &str, prefs: &ViewPrefs) {
    if let Ok(raw) = serde_json::to_string(prefs) {
        crate::client_store::set(&storage_key(library_path), &raw);
        crate::client_store::set(LAST_LIBRARY_KEY, library_path);
    }
}

/// The library path [`save`] last persisted prefs for, or `None` on a
/// first-ever visit and under SSR (where `client_store` is inert).
pub fn last_library() -> Option<String> {
    crate::client_store::get(LAST_LIBRARY_KEY).filter(|path| !path.is_empty())
}

/// Prefs for the last-browsed library, for hydrating before the library path is
/// known. `None` — rather than [`ViewPrefs::default`] — when no pointer is
/// stored, so a caller can tell "nothing was ever saved" from "defaults were
/// saved" and skip a pointless signal write.
///
/// The result is a *guess*: the viewer may since have switched libraries. The
/// landing page still reconciles against the authoritative path once a fetch
/// returns it, so a stale pointer costs the one extra fetch this exists to
/// avoid rather than leaving the wrong prefs applied.
pub fn load_last() -> Option<ViewPrefs> {
    last_library().map(|path| load(&path))
}

// SSR / server-only tests — exercise the no-persistence path that compiles
// under the default `cargo test -p omnibus-frontend --features server`. The
// `web`/`mobile` persistence lives in `client_store` behind its own cfgs and is
// unreachable here, so these assertions pin the documented SSR contract: every
// load returns defaults and save is an inert no-op.
#[cfg(all(test, not(any(feature = "web", feature = "mobile"))))]
mod ssr_tests {
    use super::*;
    use omnibus_shared::{SortDir, SortKey, ViewFilters, ViewMode};

    #[test]
    fn load_always_returns_defaults() {
        assert_eq!(load("/library/a"), ViewPrefs::default());
        assert_eq!(load(""), ViewPrefs::default());
    }

    #[test]
    fn save_is_a_noop_and_does_not_affect_subsequent_loads() {
        let prefs = ViewPrefs {
            view_mode: ViewMode::Table,
            sort_key: SortKey::Author,
            sort_dir: SortDir::Desc,
            filters: ViewFilters {
                formats: vec!["epub".into()],
                ..Default::default()
            },
            filters_open: true,
        };
        // Persisting must not change what a later load returns under SSR.
        save("/library/a", &prefs);
        assert_eq!(load("/library/a"), ViewPrefs::default());
    }

    #[test]
    fn last_library_and_load_last_are_none_under_ssr() {
        // The landing page hydrates from these before its first fetch. Under
        // SSR they must stay empty, or the server would render a grid sorted
        // differently from the first WASM paint (rule 07).
        save("/library/a", &ViewPrefs::default());
        assert_eq!(last_library(), None);
        assert_eq!(load_last(), None);
    }

    #[test]
    fn default_prefs_match_documented_shape() {
        // First-hydration markup depends on these defaults: Grid view, sorted
        // by Title ascending, no active filters, sidebar closed.
        let d = ViewPrefs::default();
        assert_eq!(d.view_mode, ViewMode::Grid);
        assert_eq!(d.sort_key, SortKey::Title);
        assert_eq!(d.sort_dir, SortDir::Asc);
        assert!(d.filters.is_empty());
        assert!(!d.filters_open);
    }
}

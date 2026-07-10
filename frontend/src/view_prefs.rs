//! Per-library view-preference persistence for [`ViewPrefs`], layered on the
//! durable [`crate::client_store`] (localStorage on web, a JSON file on mobile,
//! inert on SSR so first-hydration markup matches the WASM client). Keyed per
//! library path so each library keeps independent sort/filter state. Shape
//! lives in `omnibus-shared`.

use omnibus_shared::ViewPrefs;

const STORAGE_PREFIX: &str = "omnibus.view_prefs::";

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
pub fn save(library_path: &str, prefs: &ViewPrefs) {
    if let Ok(raw) = serde_json::to_string(prefs) {
        crate::client_store::set(&storage_key(library_path), &raw);
    }
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
            view_mode: ViewMode::Grid,
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
    fn default_prefs_match_documented_shape() {
        // First-hydration markup depends on these defaults: Table view, sorted
        // by Title ascending, no active filters, sidebar closed.
        let d = ViewPrefs::default();
        assert_eq!(d.view_mode, ViewMode::Table);
        assert_eq!(d.sort_key, SortKey::Title);
        assert_eq!(d.sort_dir, SortDir::Asc);
        assert!(d.filters.is_empty());
        assert!(!d.filters_open);
    }
}

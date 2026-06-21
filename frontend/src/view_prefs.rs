//! Per-library view-preference persistence for [`ViewPrefs`]. Web stores
//! per library-path in `localStorage`, mobile keeps an in-memory map, SSR
//! returns defaults so first-hydration markup matches the WASM client.
//! Shape lives in `omnibus-shared` so a future server endpoint can reuse it.
//! In-file `#[cfg]`-framing dividers retained per rule 05's nav-aid exception.

use omnibus_shared::ViewPrefs;

#[cfg(feature = "web")]
const STORAGE_PREFIX: &str = "omnibus.view_prefs::";

/// Load view preferences for `library_path`. Returns [`ViewPrefs::default`]
/// when no record exists, when storage is unavailable, or when the stored
/// JSON fails to parse.
pub fn load(library_path: &str) -> ViewPrefs {
    load_impl(library_path)
}

/// Persist `prefs` for `library_path`. Failures (private mode, quota, etc.)
/// are silently ignored — the UI keeps the in-memory copy regardless.
pub fn save(library_path: &str, prefs: &ViewPrefs) {
    save_impl(library_path, prefs);
}

// -----------------------------------------------------------------------------
// Web — localStorage
// -----------------------------------------------------------------------------

#[cfg(feature = "web")]
fn storage_key(library_path: &str) -> String {
    format!("{STORAGE_PREFIX}{library_path}")
}

#[cfg(feature = "web")]
fn local_storage() -> Option<web_sys::Storage> {
    web_sys::window().and_then(|w| w.local_storage().ok().flatten())
}

#[cfg(feature = "web")]
fn load_impl(library_path: &str) -> ViewPrefs {
    let Some(storage) = local_storage() else {
        return ViewPrefs::default();
    };
    let key = storage_key(library_path);
    match storage.get_item(&key) {
        Ok(Some(raw)) => serde_json::from_str(&raw).unwrap_or_default(),
        _ => ViewPrefs::default(),
    }
}

#[cfg(feature = "web")]
fn save_impl(library_path: &str, prefs: &ViewPrefs) {
    let Some(storage) = local_storage() else {
        return;
    };
    let Ok(raw) = serde_json::to_string(prefs) else {
        return;
    };
    let _ = storage.set_item(&storage_key(library_path), &raw);
}

// -----------------------------------------------------------------------------
// Mobile — process-local map; resets on cold launch.
// -----------------------------------------------------------------------------

#[cfg(feature = "mobile")]
mod mobile_store {
    use omnibus_shared::ViewPrefs;
    use std::collections::HashMap;
    use std::sync::{OnceLock, RwLock};

    fn map() -> &'static RwLock<HashMap<String, ViewPrefs>> {
        static MAP: OnceLock<RwLock<HashMap<String, ViewPrefs>>> = OnceLock::new();
        MAP.get_or_init(|| RwLock::new(HashMap::new()))
    }

    /// Read the cached prefs for `library_path` from the process-local map.
    /// Returns [`ViewPrefs::default`] when no entry exists or the lock is poisoned.
    pub fn get(library_path: &str) -> ViewPrefs {
        map()
            .read()
            .ok()
            .and_then(|g| g.get(library_path).cloned())
            .unwrap_or_default()
    }

    /// Insert/overwrite the cached prefs for `library_path` in the process-local map.
    /// Silently no-ops if the lock is poisoned; the in-memory copy resets on cold launch.
    pub fn set(library_path: &str, prefs: ViewPrefs) {
        if let Ok(mut g) = map().write() {
            g.insert(library_path.to_string(), prefs);
        }
    }

    #[cfg(test)]
    pub fn clear() {
        if let Ok(mut g) = map().write() {
            g.clear();
        }
    }
}

#[cfg(feature = "mobile")]
fn load_impl(library_path: &str) -> ViewPrefs {
    mobile_store::get(library_path)
}

#[cfg(feature = "mobile")]
fn save_impl(library_path: &str, prefs: &ViewPrefs) {
    mobile_store::set(library_path, prefs.clone());
}

// -----------------------------------------------------------------------------
// SSR / server-only build — no persistence; defaults always.
// -----------------------------------------------------------------------------

#[cfg(not(any(feature = "web", feature = "mobile")))]
fn load_impl(_library_path: &str) -> ViewPrefs {
    ViewPrefs::default()
}

#[cfg(not(any(feature = "web", feature = "mobile")))]
fn save_impl(_library_path: &str, _prefs: &ViewPrefs) {}

// -----------------------------------------------------------------------------
// Tests — only the in-memory mobile store can be exercised without a browser.
// -----------------------------------------------------------------------------

#[cfg(all(test, feature = "mobile"))]
mod tests {
    use super::*;
    use omnibus_shared::{SortDir, SortKey, ViewMode};

    #[test]
    fn returns_default_when_unset() {
        mobile_store::clear();
        let prefs = load("/library/a");
        assert_eq!(prefs, ViewPrefs::default());
    }

    #[test]
    fn round_trips_and_isolates_per_library() {
        mobile_store::clear();
        let a = ViewPrefs {
            view_mode: ViewMode::Grid,
            sort_key: SortKey::Author,
            sort_dir: SortDir::Desc,
            ..Default::default()
        };
        save("/library/a", &a);

        let b = ViewPrefs {
            view_mode: ViewMode::Table,
            sort_key: SortKey::NewestAdded,
            ..Default::default()
        };
        save("/library/b", &b);

        assert_eq!(load("/library/a"), a);
        assert_eq!(load("/library/b"), b);
        assert_eq!(load("/library/missing"), ViewPrefs::default());
    }
}

// -----------------------------------------------------------------------------
// SSR / server-only tests — exercise the no-persistence path that compiles
// under the `server` feature (the default `cargo test -p omnibus-frontend
// --features server`). The `web`/`mobile` impls live behind their own cfgs and
// are unreachable here, so these assertions pin the documented SSR contract:
// every call returns defaults and `save` is an inert no-op.
// -----------------------------------------------------------------------------
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

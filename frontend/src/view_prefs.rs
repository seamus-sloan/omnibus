//! Per-library view-preference persistence for [`ViewPrefs`].
//!
//! Web persists to `localStorage` keyed by library path; mobile keeps an
//! in-memory map (resets on cold launch — explicit MVP choice in F1.3 plan,
//! to be replaced when a server-backed `/api/user/view-prefs` lands); server
//! (SSR) always returns defaults so the rendered markup matches what the
//! WASM client paints on first hydration.
//!
//! Shape lives in `omnibus-shared` so a future server endpoint can reuse the
//! same JSON contract.

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

    pub fn get(library_path: &str) -> ViewPrefs {
        map()
            .read()
            .ok()
            .and_then(|g| g.get(library_path).cloned())
            .unwrap_or_default()
    }

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
        let mut a = ViewPrefs::default();
        a.view_mode = ViewMode::Grid;
        a.sort_key = SortKey::Author;
        a.sort_dir = SortDir::Desc;
        save("/library/a", &a);

        let mut b = ViewPrefs::default();
        b.view_mode = ViewMode::Table;
        b.sort_key = SortKey::NewestAdded;
        save("/library/b", &b);

        assert_eq!(load("/library/a"), a);
        assert_eq!(load("/library/b"), b);
        assert_eq!(load("/library/missing"), ViewPrefs::default());
    }
}

//! Reading-position persistence — saved EPUB CFI per book. Web stores
//! per-UUID in `localStorage`, mobile keeps an in-memory map, SSR is a
//! no-op. Offline / first-paint cache for the progress-sync endpoint:
//! reader reads sync, saves write here AND fire-and-forget POST.
//! In-file `#[cfg]`-framing dividers retained per rule 05's nav-aid exception.

#[cfg(feature = "web")]
const STORAGE_PREFIX: &str = "omn.reading::";

/// Load the saved EPUB CFI for `uuid`. Returns `None` when no record exists,
/// when storage is unavailable, or under SSR.
pub fn load(uuid: &str) -> Option<String> {
    load_impl(uuid)
}

/// Persist `cfi` as the reading position for `uuid`. Failures (private mode,
/// quota, etc.) are silently ignored.
pub fn save(uuid: &str, cfi: &str) {
    save_impl(uuid, cfi);
}

// -----------------------------------------------------------------------------
// Web — localStorage
// -----------------------------------------------------------------------------

#[cfg(feature = "web")]
fn storage_key(uuid: &str) -> String {
    format!("{STORAGE_PREFIX}{uuid}")
}

#[cfg(feature = "web")]
fn local_storage() -> Option<web_sys::Storage> {
    web_sys::window().and_then(|w| w.local_storage().ok().flatten())
}

#[cfg(feature = "web")]
fn load_impl(uuid: &str) -> Option<String> {
    let storage = local_storage()?;
    storage.get_item(&storage_key(uuid)).ok().flatten()
}

#[cfg(feature = "web")]
fn save_impl(uuid: &str, cfi: &str) {
    let Some(storage) = local_storage() else {
        return;
    };
    let _ = storage.set_item(&storage_key(uuid), cfi);
}

// -----------------------------------------------------------------------------
// Mobile — process-local map; resets on cold launch.
// -----------------------------------------------------------------------------

#[cfg(feature = "mobile")]
mod mobile_store {
    use std::collections::HashMap;
    use std::sync::{OnceLock, RwLock};

    fn map() -> &'static RwLock<HashMap<String, String>> {
        static MAP: OnceLock<RwLock<HashMap<String, String>>> = OnceLock::new();
        MAP.get_or_init(|| RwLock::new(HashMap::new()))
    }

    /// Read the cached CFI for `uuid` from the process-local map.
    /// Returns `None` when no entry exists or the lock is poisoned.
    pub fn get(uuid: &str) -> Option<String> {
        map().read().ok().and_then(|g| g.get(uuid).cloned())
    }

    /// Insert/overwrite the cached CFI for `uuid` in the process-local map.
    /// Silently no-ops if the lock is poisoned; the in-memory copy resets on cold launch.
    pub fn set(uuid: &str, cfi: String) {
        if let Ok(mut g) = map().write() {
            g.insert(uuid.to_string(), cfi);
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
fn load_impl(uuid: &str) -> Option<String> {
    mobile_store::get(uuid)
}

#[cfg(feature = "mobile")]
fn save_impl(uuid: &str, cfi: &str) {
    mobile_store::set(uuid, cfi.to_string());
}

// -----------------------------------------------------------------------------
// SSR / server-only build — no persistence.
// -----------------------------------------------------------------------------

#[cfg(not(any(feature = "web", feature = "mobile")))]
fn load_impl(_uuid: &str) -> Option<String> {
    None
}

#[cfg(not(any(feature = "web", feature = "mobile")))]
fn save_impl(_uuid: &str, _cfi: &str) {}

// -----------------------------------------------------------------------------
// Tests — only the in-memory mobile store can be exercised without a browser.
// -----------------------------------------------------------------------------

#[cfg(all(test, feature = "mobile"))]
mod tests {
    use super::*;

    #[test]
    fn returns_none_when_unset() {
        mobile_store::clear();
        assert_eq!(load("book-a"), None);
    }

    #[test]
    fn round_trips_and_isolates_per_uuid() {
        mobile_store::clear();
        save("book-a", "epubcfi(/6/4[chap01]!/4/2/1:0)");
        save("book-b", "epubcfi(/6/8[chap04]!/4/10/2:42)");

        assert_eq!(
            load("book-a"),
            Some("epubcfi(/6/4[chap01]!/4/2/1:0)".to_string())
        );
        assert_eq!(
            load("book-b"),
            Some("epubcfi(/6/8[chap04]!/4/10/2:42)".to_string())
        );
        assert_eq!(load("book-missing"), None);
    }

    #[test]
    fn save_overwrites_prior_position() {
        mobile_store::clear();
        save("book-a", "epubcfi(/6/4!/4/2/1:0)");
        save("book-a", "epubcfi(/6/12!/4/8/3:7)");
        assert_eq!(load("book-a"), Some("epubcfi(/6/12!/4/8/3:7)".to_string()));
    }
}

// -----------------------------------------------------------------------------
// SSR / server-only tests — exercise the no-persistence path that compiles
// under the `server` feature (the default `cargo test -p omnibus-frontend
// --features server`). The `web`/`mobile` impls live behind their own cfgs and
// are unreachable here, so these assertions pin the documented SSR contract:
// every load returns `None` and `save` is an inert no-op.
// -----------------------------------------------------------------------------
#[cfg(all(test, not(any(feature = "web", feature = "mobile"))))]
mod ssr_tests {
    use super::*;

    #[test]
    fn load_always_returns_none() {
        assert_eq!(load("book-a"), None);
        assert_eq!(load(""), None);
    }

    #[test]
    fn save_is_a_noop_and_does_not_affect_subsequent_loads() {
        // Persisting must not change what a later load returns under SSR.
        save("book-a", "epubcfi(/6/4[chap01]!/4/2/1:0)");
        assert_eq!(load("book-a"), None);
    }
}

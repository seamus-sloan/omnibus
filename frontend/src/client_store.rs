//! Durable client-local key→value store shared by [`crate::view_prefs`] and
//! [`crate::index_prefs`]: `localStorage` on web, an in-memory map mirrored to
//! a JSON file under the app data dir on mobile (survives cold launch), inert
//! on SSR so first-hydration markup keeps its defaults.

/// Read the string stored under `key`, or `None` if absent/unavailable.
pub fn get(key: &str) -> Option<String> {
    get_impl(key)
}

/// Persist `value` under `key`, best-effort (storage failures are ignored).
pub fn set(key: &str, value: &str) {
    set_impl(key, value);
}

// Web — localStorage.
#[cfg(feature = "web")]
fn local_storage() -> Option<web_sys::Storage> {
    web_sys::window().and_then(|w| w.local_storage().ok().flatten())
}

#[cfg(feature = "web")]
fn get_impl(key: &str) -> Option<String> {
    local_storage()?.get_item(key).ok().flatten()
}

#[cfg(feature = "web")]
fn set_impl(key: &str, value: &str) {
    if let Some(storage) = local_storage() {
        let _ = storage.set_item(key, value);
    }
}

// Mobile — an in-memory map loaded once from `prefs.json` under the app data
// dir and written through on every `set`, so entries persist across cold
// launches (unlike the old process-local map).
#[cfg(feature = "mobile")]
mod mobile_store {
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::{OnceLock, RwLock};

    /// On-disk path for the persisted map, or `None` when no writable app data
    /// dir is available (see `crate::data::app_dirs::data_dir`).
    fn prefs_path() -> Option<PathBuf> {
        crate::data::app_dirs::data_dir().map(|d| d.join("prefs.json"))
    }

    /// Parse the on-disk JSON object into a key→value map. A missing, empty, or
    /// malformed file yields an empty map (treated as "nothing saved").
    fn parse_map(raw: &str) -> HashMap<String, String> {
        serde_json::from_str(raw).unwrap_or_default()
    }

    /// The process-global map, seeded once from disk on first access.
    fn map() -> &'static RwLock<HashMap<String, String>> {
        static MAP: OnceLock<RwLock<HashMap<String, String>>> = OnceLock::new();
        MAP.get_or_init(|| {
            let loaded = prefs_path()
                .and_then(|p| std::fs::read_to_string(p).ok())
                .map(|raw| parse_map(&raw))
                .unwrap_or_default();
            RwLock::new(loaded)
        })
    }

    /// Read `key` from the in-memory map.
    pub fn get(key: &str) -> Option<String> {
        map().read().ok().and_then(|m| m.get(key).cloned())
    }

    /// Insert/overwrite `key` and flush the whole map to disk (best-effort). A
    /// write failure is logged and ignored — the in-memory copy still serves
    /// this session, so the only cost is losing the value on next launch.
    ///
    /// The flush is atomic (write a sibling temp file, then rename over the
    /// target) so a crash mid-write can't leave a half-written `prefs.json`
    /// that fails to parse on next launch and drops every stored pref.
    pub fn set(key: &str, value: &str) {
        #[cfg(test)]
        SET_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let snapshot = {
            let Ok(mut m) = map().write() else {
                return;
            };
            m.insert(key.to_string(), value.to_string());
            serde_json::to_string(&*m).ok()
        };
        if let (Some(json), Some(path)) = (snapshot, prefs_path()) {
            let tmp = path.with_extension("json.tmp");
            let flush = std::fs::write(&tmp, json).and_then(|()| std::fs::rename(&tmp, &path));
            if let Err(e) = flush {
                tracing::warn!(error = %e, path = %path.display(), "could not persist client prefs");
            }
        }
    }

    /// Test-only count of [`set`] calls, so a caller-level test can observe a
    /// dedup-write skip (a call that never reaches this module) without
    /// reaching into the private map itself.
    #[cfg(test)]
    pub(super) static SET_CALLS: std::sync::atomic::AtomicUsize =
        std::sync::atomic::AtomicUsize::new(0);

    /// Test-only reset: empties the map and zeroes the call counter, so each
    /// test starts from a clean slate despite sharing this process-global
    /// store.
    #[cfg(test)]
    pub(super) fn reset_for_test() {
        if let Ok(mut m) = map().write() {
            m.clear();
        }
        SET_CALLS.store(0, std::sync::atomic::Ordering::SeqCst);
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn parse_map_reads_a_json_object() {
            let m = parse_map(r#"{"a":"1","b":"two"}"#);
            assert_eq!(m.get("a"), Some(&"1".to_string()));
            assert_eq!(m.get("b"), Some(&"two".to_string()));
        }

        #[test]
        fn parse_map_returns_empty_for_empty_or_garbage() {
            assert!(parse_map("").is_empty());
            assert!(parse_map("not json").is_empty());
            assert!(parse_map("[1,2,3]").is_empty());
        }
    }
}

/// Test-only: reset the mobile backend's in-memory map and [`set`] call
/// counter. Exposed at module scope so cross-file test modules (e.g.
/// [`crate::view_prefs`]'s) can reach it without `mobile_store` itself being
/// `pub`.
#[cfg(all(test, feature = "mobile"))]
pub fn reset_for_test() {
    mobile_store::reset_for_test();
}

/// Test-only: total number of [`set`] calls the mobile backend has serviced
/// since the last [`reset_for_test`].
#[cfg(all(test, feature = "mobile"))]
pub fn set_call_count_for_test() -> usize {
    mobile_store::SET_CALLS.load(std::sync::atomic::Ordering::SeqCst)
}

#[cfg(feature = "mobile")]
fn get_impl(key: &str) -> Option<String> {
    mobile_store::get(key)
}

#[cfg(feature = "mobile")]
fn set_impl(key: &str, value: &str) {
    mobile_store::set(key, value);
}

// SSR / server-only build — no persistence; `get` is always `None` and `set`
// is inert, so components render their target-agnostic defaults (rule 07).
#[cfg(not(any(feature = "web", feature = "mobile")))]
fn get_impl(_key: &str) -> Option<String> {
    None
}

#[cfg(not(any(feature = "web", feature = "mobile")))]
fn set_impl(_key: &str, _value: &str) {}

// SSR tests — pin the documented no-persistence contract that compiles under
// the default `--features server` test run.
#[cfg(all(test, not(any(feature = "web", feature = "mobile"))))]
mod ssr_tests {
    use super::*;

    #[test]
    fn get_is_always_none_and_set_is_a_noop() {
        set("omnibus.k", "value");
        assert_eq!(get("omnibus.k"), None);
    }
}

//! Durable client-local key→value store — a small `localStorage`-shaped
//! primitive shared by [`crate::view_prefs`] and [`crate::index_prefs`]. Web
//! reads/writes `localStorage`; mobile mirrors an in-memory map to a JSON file
//! under the app data dir so values survive a cold launch; the SSR/server
//! build is inert so first-hydration markup keeps its defaults. Kept flat:
//! each impl is a `#[cfg]` variant of one signature.

/// Read the string stored under `key`, or `None` when it is absent or storage
/// is unavailable.
pub fn get(key: &str) -> Option<String> {
    get_impl(key)
}

/// Persist `value` under `key`, best-effort. Failures (private mode, quota, an
/// unwritable data dir) are silently ignored — callers keep their in-memory copy.
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
    pub fn set(key: &str, value: &str) {
        let snapshot = {
            let Ok(mut m) = map().write() else {
                return;
            };
            m.insert(key.to_string(), value.to_string());
            serde_json::to_string(&*m).ok()
        };
        if let (Some(json), Some(path)) = (snapshot, prefs_path()) {
            if let Err(e) = std::fs::write(&path, json) {
                tracing::warn!(error = %e, path = %path.display(), "could not persist client prefs");
            }
        }
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

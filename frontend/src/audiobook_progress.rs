//! Listening-position persistence — the saved `currentTime` (seconds,
//! float) for each audiobook. Mirrors [`crate::reader_progress`] in shape
//! (web `localStorage`, mobile in-memory map, server no-op) and caches the
//! first-paint position the listen page reconciles against `POST /api/progress`.
//! Kept flat: each impl is a `#[cfg]` variant of one signature (no submodules).

#[cfg(feature = "web")]
const STORAGE_PREFIX: &str = "omn.listening::";
/// Load the saved position (seconds) for `uuid`. Returns `None` when no
/// record exists, when storage is unavailable, or under SSR.
pub fn load(uuid: &str) -> Option<f64> {
    load_impl(uuid)
}

/// Persist `seconds` as the listening position for `uuid`. Failures
/// (private mode, quota, etc.) are silently ignored.
pub fn save(uuid: &str, seconds: f64) {
    save_impl(uuid, seconds);
}

/// Load the saved playback-rate preference for `uuid`. Defaults to
/// `1.0` when unset. Each book remembers its own speed.
pub fn load_rate(uuid: &str) -> f64 {
    load_rate_impl(uuid).unwrap_or(1.0)
}

/// Persist the playback-rate preference for `uuid`.
pub fn save_rate(uuid: &str, rate: f64) {
    save_rate_impl(uuid, rate);
}

// Web — localStorage.
#[cfg(feature = "web")]
fn storage_key(uuid: &str) -> String {
    format!("{STORAGE_PREFIX}{uuid}")
}

#[cfg(feature = "web")]
fn local_storage() -> Option<web_sys::Storage> {
    web_sys::window().and_then(|w| w.local_storage().ok().flatten())
}

#[cfg(feature = "web")]
fn load_impl(uuid: &str) -> Option<f64> {
    let storage = local_storage()?;
    let raw = storage.get_item(&storage_key(uuid)).ok().flatten()?;
    raw.parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && *v >= 0.0)
}

#[cfg(feature = "web")]
fn save_impl(uuid: &str, seconds: f64) {
    let Some(storage) = local_storage() else {
        return;
    };
    let _ = storage.set_item(&storage_key(uuid), &format!("{seconds}"));
}

#[cfg(feature = "web")]
fn rate_key(uuid: &str) -> String {
    format!("{STORAGE_PREFIX}rate::{uuid}")
}

#[cfg(feature = "web")]
fn load_rate_impl(uuid: &str) -> Option<f64> {
    let storage = local_storage()?;
    storage
        .get_item(&rate_key(uuid))
        .ok()
        .flatten()?
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && *v > 0.0)
}

#[cfg(feature = "web")]
fn save_rate_impl(uuid: &str, rate: f64) {
    let Some(storage) = local_storage() else {
        return;
    };
    let _ = storage.set_item(&rate_key(uuid), &format!("{rate}"));
}

// Mobile — process-local map; resets on cold launch.
#[cfg(feature = "mobile")]
mod mobile_store {
    use std::collections::HashMap;
    use std::sync::{OnceLock, RwLock};

    fn map() -> &'static RwLock<HashMap<String, f64>> {
        static MAP: OnceLock<RwLock<HashMap<String, f64>>> = OnceLock::new();
        MAP.get_or_init(|| RwLock::new(HashMap::new()))
    }

    /// Read the cached position (seconds) for `uuid` from the process-local map.
    /// Returns `None` when no entry exists or the lock is poisoned.
    pub fn get(uuid: &str) -> Option<f64> {
        map().read().ok().and_then(|g| g.get(uuid).copied())
    }

    /// Insert/overwrite the cached position (seconds) for `uuid` in the process-local map.
    /// Silently no-ops if the lock is poisoned; the in-memory copy resets on cold launch.
    pub fn set(uuid: &str, seconds: f64) {
        if let Ok(mut g) = map().write() {
            g.insert(uuid.to_string(), seconds);
        }
    }

    fn rate_key(uuid: &str) -> String {
        format!("rate::{uuid}")
    }
    /// Read the cached playback-rate preference for `uuid` from the process-local map.
    /// Returns `None` when no entry exists or the lock is poisoned.
    pub fn get_rate(uuid: &str) -> Option<f64> {
        map()
            .read()
            .ok()
            .and_then(|g| g.get(&rate_key(uuid)).copied())
    }
    /// Insert/overwrite the cached playback-rate preference for `uuid` in the process-local map.
    /// Silently no-ops if the lock is poisoned; the in-memory copy resets on cold launch.
    pub fn set_rate(uuid: &str, r: f64) {
        if let Ok(mut g) = map().write() {
            g.insert(rate_key(uuid), r);
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
fn load_impl(uuid: &str) -> Option<f64> {
    mobile_store::get(uuid)
}

#[cfg(feature = "mobile")]
fn save_impl(uuid: &str, seconds: f64) {
    mobile_store::set(uuid, seconds);
}

#[cfg(feature = "mobile")]
fn load_rate_impl(uuid: &str) -> Option<f64> {
    mobile_store::get_rate(uuid)
}

#[cfg(feature = "mobile")]
fn save_rate_impl(uuid: &str, rate: f64) {
    mobile_store::set_rate(uuid, rate);
}

// SSR / server-only build — no persistence.
#[cfg(not(any(feature = "web", feature = "mobile")))]
fn load_impl(_uuid: &str) -> Option<f64> {
    None
}

#[cfg(not(any(feature = "web", feature = "mobile")))]
fn save_impl(_uuid: &str, _seconds: f64) {}

#[cfg(not(any(feature = "web", feature = "mobile")))]
fn load_rate_impl(_uuid: &str) -> Option<f64> {
    None
}

#[cfg(not(any(feature = "web", feature = "mobile")))]
fn save_rate_impl(_uuid: &str, _rate: f64) {}

// Tests — only the in-memory mobile store can be exercised without a browser.
#[cfg(all(test, feature = "mobile"))]
mod tests {
    use super::*;

    #[test]
    fn returns_none_when_unset() {
        mobile_store::clear();
        assert_eq!(load("book-a"), None);
    }

    #[test]
    fn round_trips_per_uuid() {
        mobile_store::clear();
        save("book-a", 12.5);
        save("book-b", 600.0);
        assert_eq!(load("book-a"), Some(12.5));
        assert_eq!(load("book-b"), Some(600.0));
    }

    #[test]
    fn rate_defaults_and_persists_per_book() {
        mobile_store::clear();
        assert!((load_rate("book-a") - 1.0).abs() < f64::EPSILON);
        save_rate("book-a", 1.5);
        save_rate("book-b", 2.0);
        assert!((load_rate("book-a") - 1.5).abs() < f64::EPSILON);
        assert!((load_rate("book-b") - 2.0).abs() < f64::EPSILON);
        assert!((load_rate("book-c") - 1.0).abs() < f64::EPSILON);
    }
}

#[cfg(all(test, not(any(feature = "web", feature = "mobile"))))]
mod ssr_tests {
    use super::*;

    #[test]
    fn load_always_returns_none() {
        assert_eq!(load("book-a"), None);
    }

    #[test]
    fn save_is_a_noop_under_ssr() {
        save("book-a", 100.0);
        assert_eq!(load("book-a"), None);
    }

    #[test]
    fn rate_defaults_to_one() {
        assert!((load_rate("any-book") - 1.0).abs() < f64::EPSILON);
    }
}

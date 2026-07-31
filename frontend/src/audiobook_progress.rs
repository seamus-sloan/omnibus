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

/// Result of reconciling the server preference with the scoped local fallback.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlaybackRateResolution {
    pub playback_rate: f64,
    pub seed_server: bool,
}

/// Resolve a playback rate without treating a failed fetch as an empty server.
pub fn resolve_rate(
    server_rate: Result<Option<f64>, ()>,
    local_rate: Option<f64>,
) -> PlaybackRateResolution {
    match server_rate {
        Ok(Some(playback_rate)) => PlaybackRateResolution {
            playback_rate,
            seed_server: false,
        },
        Ok(None) => match local_rate {
            Some(playback_rate) => PlaybackRateResolution {
                playback_rate,
                seed_server: true,
            },
            None => PlaybackRateResolution {
                playback_rate: 1.0,
                seed_server: false,
            },
        },
        Err(_) => PlaybackRateResolution {
            playback_rate: local_rate.unwrap_or(1.0),
            seed_server: false,
        },
    }
}

/// Whether an asynchronous playback load still belongs to the active account/book.
pub fn playback_load_matches(
    active_uuid: Option<&str>,
    active_user_id: Option<i64>,
    expected_uuid: &str,
    expected_user_id: Option<i64>,
) -> bool {
    active_uuid == Some(expected_uuid) && active_user_id == expected_user_id
}

fn rate_storage_key(user_id: i64, uuid: &str) -> String {
    format!("omn.listening.rate::{user_id}::{uuid}")
}

fn parse_rate(raw: &str) -> Option<f64> {
    raw.parse::<f64>().ok().filter(|rate| {
        rate.is_finite()
            && (omnibus_shared::MIN_AUDIOBOOK_PLAYBACK_RATE
                ..=omnibus_shared::MAX_AUDIOBOOK_PLAYBACK_RATE)
                .contains(rate)
    })
}

/// Load the best-effort local playback-rate fallback for `(user, book)`.
pub fn load_rate(user_id: i64, uuid: &str) -> Option<f64> {
    crate::client_store::get(&rate_storage_key(user_id, uuid))
        .as_deref()
        .and_then(parse_rate)
}

/// Persist the best-effort local playback-rate fallback for `(user, book)`.
pub fn save_rate(user_id: i64, uuid: &str, rate: f64) {
    crate::client_store::set(&rate_storage_key(user_id, uuid), &rate.to_string());
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

// Mobile — process-local map for synchronous reads, write-through to the
// offline SQLite store so positions survive a cold launch.
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

    /// Insert/overwrite the cached position (seconds) for `uuid`, mirroring
    /// the write into the durable offline store (best-effort, off-thread).
    pub fn set(uuid: &str, seconds: f64) {
        if let Ok(mut g) = map().write() {
            g.insert(uuid.to_string(), seconds);
        }
        if let Some(store) = crate::offline::store::store() {
            store.kv_put(
                &crate::offline::cache::keys::audio_pos(uuid),
                format!("{seconds}"),
            );
        }
    }

    /// Seed the map from the durable rows (cold-start hydration).
    pub fn seed(entries: Vec<(String, f64)>) {
        if let Ok(mut g) = map().write() {
            for (uuid, seconds) in entries {
                g.entry(uuid).or_insert(seconds);
            }
        }
    }

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

/// Hydrate the in-memory map from the offline store's durable rows. Called
/// once from `offline::init` before launch (blocking read; no runtime yet).
#[cfg(feature = "mobile")]
pub fn hydrate_from_offline_store() {
    let Some(store) = crate::offline::store::store() else {
        return;
    };
    let prefix = crate::offline::cache::keys::audio_pos("");
    let entries = store
        .kv_prefix_blocking(&prefix)
        .into_iter()
        .filter_map(|(key, raw)| {
            let uuid = key.strip_prefix(prefix.as_str())?;
            let seconds = raw.parse::<f64>().ok()?;
            (seconds.is_finite() && seconds >= 0.0).then(|| (uuid.to_string(), seconds))
        })
        .collect();
    mobile_store::seed(entries);
}

/// Clear the process-local mobile audiobook-position cache. Used when a
/// different account signs in so resume fallback cannot bleed across users.
#[cfg(feature = "mobile")]
pub(crate) fn clear_for_user_switch() {
    mobile_store::clear();
}

// SSR / server-only build — no persistence.
#[cfg(not(any(feature = "web", feature = "mobile")))]
fn load_impl(_uuid: &str) -> Option<f64> {
    None
}

#[cfg(not(any(feature = "web", feature = "mobile")))]
fn save_impl(_uuid: &str, _seconds: f64) {}

// Tests — only the in-memory mobile store can be exercised without a browser.
#[cfg(all(test, feature = "mobile"))]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // These tests share one process-global map and each `clear()`s it, so
    // running them in parallel wipes each other. Serialize them.
    static TEST_GUARD: Mutex<()> = Mutex::new(());

    #[test]
    fn returns_none_when_unset() {
        let _g = TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        mobile_store::clear();
        assert_eq!(load("book-a"), None);
    }

    #[test]
    fn round_trips_per_uuid() {
        let _g = TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        mobile_store::clear();
        save("book-a", 12.5);
        save("book-b", 600.0);
        assert_eq!(load("book-a"), Some(12.5));
        assert_eq!(load("book-b"), Some(600.0));
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
    fn rate_is_unset() {
        assert_eq!(load_rate(1, "any-book"), None);
    }
}

#[cfg(test)]
mod rate_resolution_tests {
    use super::*;

    #[test]
    fn server_rate_wins_over_local_rate() {
        assert_eq!(
            resolve_rate(Ok(Some(1.5)), Some(2.0)),
            PlaybackRateResolution {
                playback_rate: 1.5,
                seed_server: false,
            }
        );
    }

    #[test]
    fn scoped_local_rate_seeds_an_empty_server() {
        assert_eq!(
            resolve_rate(Ok(None), Some(1.5)),
            PlaybackRateResolution {
                playback_rate: 1.5,
                seed_server: true,
            }
        );
    }

    #[test]
    fn missing_server_and_local_rates_use_default_without_seeding() {
        assert_eq!(
            resolve_rate(Ok(None), None),
            PlaybackRateResolution {
                playback_rate: 1.0,
                seed_server: false,
            }
        );
    }

    #[test]
    fn server_fetch_failure_uses_local_rate_without_seeding() {
        assert_eq!(
            resolve_rate(Err(()), Some(1.8)),
            PlaybackRateResolution {
                playback_rate: 1.8,
                seed_server: false,
            }
        );
    }

    #[test]
    fn rate_storage_key_is_scoped_by_user_and_ignores_legacy_shape() {
        let key = rate_storage_key(42, "book-a");
        assert_eq!(key, "omn.listening.rate::42::book-a");
        assert_ne!(key, "omn.listening.rate::book-a");
        assert_ne!(key, rate_storage_key(7, "book-a"));
        assert_ne!(key, rate_storage_key(42, "book-b"));
    }

    #[test]
    fn parse_rate_rejects_invalid_local_values() {
        for raw in ["NaN", "inf", "0.49", "3.01", "not-a-number"] {
            assert_eq!(parse_rate(raw), None);
        }
        assert_eq!(parse_rate("2.25"), Some(2.25));
    }

    #[test]
    fn playback_load_guard_rejects_same_book_for_different_user() {
        assert!(!playback_load_matches(
            Some("book-a"),
            Some(2),
            "book-a",
            Some(1),
        ));
        assert!(playback_load_matches(
            Some("book-a"),
            Some(1),
            "book-a",
            Some(1),
        ));
    }
}

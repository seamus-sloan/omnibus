//! Provider API keys stored in `settings`: Hardcover and Google Books.
//! Each is one [`SecretKeySpec`] instance sharing the get/set/effective/
//! status/seed-from-env shape; [`provider_keys`] resolves both at once for
//! the metadata-search ladder.

use sqlx::SqlitePool;

use omnibus_shared::{
    GoogleBooksKeyStatus, HardcoverKeyStatus, GOOGLE_BOOKS_API_KEY_MAX_LEN,
    HARDCOVER_API_KEY_MAX_LEN,
};

use super::secret_key::SecretKeySpec;
use super::SettingsError;

/// `settings` KV key for the Hardcover API token. Stored directly (not via
/// the [`super::Settings`] struct) so saving it never triggers the scan-root
/// reconciliation `set_settings` runs.
const HARDCOVER_API_KEY_KEY: &str = "hardcover_api_key";
/// `settings` KV key for the Google Books API key. Stored directly (not via
/// the [`super::Settings`] struct) so saving it never triggers the scan-root
/// reconciliation `set_settings` runs.
const GOOGLE_BOOKS_API_KEY_KEY: &str = "google_books_api_key";

/// The server-wide Hardcover API token, as a [`SecretKeySpec`]. Powers F3.3
/// "Readers also enjoyed" suggestions.
const HARDCOVER_KEY: SecretKeySpec = SecretKeySpec {
    kv_key: HARDCOVER_API_KEY_KEY,
    env_var: "HARDCOVER_API_KEY",
    max_len: HARDCOVER_API_KEY_MAX_LEN,
    label: "hardcover api key",
};

/// The server-wide Google Books API key, as a [`SecretKeySpec`]. Keeps ISBN
/// check-ins resolving past the shared anonymous quota.
const GOOGLE_BOOKS_KEY: SecretKeySpec = SecretKeySpec {
    kv_key: GOOGLE_BOOKS_API_KEY_KEY,
    env_var: "GOOGLE_BOOKS_API_KEY",
    max_len: GOOGLE_BOOKS_API_KEY_MAX_LEN,
    label: "google books api key",
};

/// Read the Hardcover API key saved in `settings`, or `None` when unset/blank.
/// This is the raw secret — callers serving it to a client MUST mask it.
pub async fn get_hardcover_api_key(pool: &SqlitePool) -> Result<Option<String>, SettingsError> {
    HARDCOVER_KEY.get(pool).await
}

/// Persist (or clear, when `None`/blank) the Hardcover API key in `settings`.
/// Rejects tokens longer than [`HARDCOVER_API_KEY_MAX_LEN`] with
/// [`SettingsError::Validation`] before the write.
pub async fn set_hardcover_api_key(
    pool: &SqlitePool,
    key: Option<&str>,
) -> Result<(), SettingsError> {
    HARDCOVER_KEY.set(pool, key).await
}

/// The effective Hardcover key: the saved settings value wins; the
/// `HARDCOVER_API_KEY` env var is the fallback. `None` (feature disabled) when
/// neither is set.
pub async fn effective_hardcover_api_key(
    pool: &SqlitePool,
) -> Result<Option<String>, SettingsError> {
    HARDCOVER_KEY.effective(pool).await
}

/// Every provider key the metadata-search ladder can use, resolved
/// settings-over-env in one call. The scan routes want all of them at once —
/// which providers are reachable is the ladder's business, not each handler's.
pub async fn provider_keys(
    pool: &SqlitePool,
) -> Result<crate::metadata_lookup::ProviderKeys, SettingsError> {
    Ok(crate::metadata_lookup::ProviderKeys {
        googlebooks: effective_google_books_api_key(pool).await?,
        hardcover: effective_hardcover_api_key(pool).await?,
    })
}

/// Masked status of the server-wide Hardcover key (settings wins over env).
/// Never returns the raw key — only a short masked preview.
pub async fn hardcover_key_status(pool: &SqlitePool) -> Result<HardcoverKeyStatus, SettingsError> {
    let s = HARDCOVER_KEY.status(pool).await?;
    Ok(HardcoverKeyStatus {
        configured: s.configured,
        masked: s.masked,
        source: s.source,
    })
}

/// Boot hook: seed the Hardcover key from `HARDCOVER_API_KEY` **only** when no
/// value is already saved (settings wins across restarts).
pub async fn seed_hardcover_key_from_env(pool: &SqlitePool) -> Result<(), SettingsError> {
    HARDCOVER_KEY.seed_from_env(pool).await
}

/// Read the Google Books API key saved in `settings`, or `None` when
/// unset/blank. This is the raw secret — callers serving it to a client MUST
/// mask it.
pub async fn get_google_books_api_key(pool: &SqlitePool) -> Result<Option<String>, SettingsError> {
    GOOGLE_BOOKS_KEY.get(pool).await
}

/// Persist (or clear, when `None`/blank) the Google Books API key in
/// `settings`. Rejects keys longer than [`GOOGLE_BOOKS_API_KEY_MAX_LEN`] with
/// [`SettingsError::Validation`] before the write.
pub async fn set_google_books_api_key(
    pool: &SqlitePool,
    key: Option<&str>,
) -> Result<(), SettingsError> {
    GOOGLE_BOOKS_KEY.set(pool, key).await
}

/// The effective Google Books key: the saved settings value wins; the
/// `GOOGLE_BOOKS_API_KEY` env var is the fallback. `None` (keyless, shared
/// anonymous quota) when neither is set.
pub async fn effective_google_books_api_key(
    pool: &SqlitePool,
) -> Result<Option<String>, SettingsError> {
    GOOGLE_BOOKS_KEY.effective(pool).await
}

/// Masked status of the server-wide Google Books key (settings wins over env).
/// Never returns the raw key — only a short masked preview.
pub async fn google_books_key_status(
    pool: &SqlitePool,
) -> Result<GoogleBooksKeyStatus, SettingsError> {
    let s = GOOGLE_BOOKS_KEY.status(pool).await?;
    Ok(GoogleBooksKeyStatus {
        configured: s.configured,
        masked: s.masked,
        source: s.source,
    })
}

/// Boot hook: seed the Google Books key from `GOOGLE_BOOKS_API_KEY` **only**
/// when no value is already saved (settings wins across restarts).
pub async fn seed_google_books_key_from_env(pool: &SqlitePool) -> Result<(), SettingsError> {
    GOOGLE_BOOKS_KEY.seed_from_env(pool).await
}

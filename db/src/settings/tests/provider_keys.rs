//! The Hardcover and Google Books API keys: set/get/clear, the length
//! cap, the settings-over-env `effective_*` resolution, seed-only-when-
//! unset, and the masked status report each surfaces.

use super::super::*;
use crate::pool::init_db;
use crate::test_support::EnvVarGuard;

#[tokio::test]
async fn hardcover_key_set_get_and_clear_roundtrips() {
    let _env = EnvVarGuard::set("HARDCOVER_API_KEY", None);
    let pool = init_db("sqlite::memory:").await.unwrap();
    assert_eq!(get_hardcover_api_key(&pool).await.unwrap(), None);

    set_hardcover_api_key(&pool, Some("  hc_secret  "))
        .await
        .unwrap();
    assert_eq!(
        get_hardcover_api_key(&pool).await.unwrap().as_deref(),
        Some("hc_secret")
    );

    // Blank clears (treated as None).
    set_hardcover_api_key(&pool, Some("   ")).await.unwrap();
    assert_eq!(get_hardcover_api_key(&pool).await.unwrap(), None);
}

#[tokio::test]
async fn set_hardcover_api_key_accepts_value_at_max_len() {
    let _env = EnvVarGuard::set("HARDCOVER_API_KEY", None);
    let pool = init_db("sqlite::memory:").await.unwrap();
    let at_limit = "a".repeat(HARDCOVER_API_KEY_MAX_LEN);
    set_hardcover_api_key(&pool, Some(&at_limit)).await.unwrap();
    assert_eq!(
        get_hardcover_api_key(&pool).await.unwrap().as_deref(),
        Some(at_limit.as_str())
    );
}

#[tokio::test]
async fn set_hardcover_api_key_rejects_value_over_max_len() {
    let _env = EnvVarGuard::set("HARDCOVER_API_KEY", None);
    let pool = init_db("sqlite::memory:").await.unwrap();
    let over_limit = "a".repeat(HARDCOVER_API_KEY_MAX_LEN + 1);
    let err = set_hardcover_api_key(&pool, Some(&over_limit))
        .await
        .expect_err("over-limit value should be rejected");
    match &err {
        SettingsError::Validation(msg) => assert!(
            msg.contains(&HARDCOVER_API_KEY_MAX_LEN.to_string()),
            "error message should name the cap: {msg}"
        ),
        other => panic!("expected SettingsError::Validation, got {other:?}"),
    }
    // Validation must short-circuit before the KV write.
    assert_eq!(get_hardcover_api_key(&pool).await.unwrap(), None);
}

#[tokio::test]
async fn effective_hardcover_key_prefers_saved_over_env() {
    let _env = EnvVarGuard::set("HARDCOVER_API_KEY", Some("env-key"));
    let pool = init_db("sqlite::memory:").await.unwrap();
    set_hardcover_api_key(&pool, Some("settings-key"))
        .await
        .unwrap();
    assert_eq!(
        effective_hardcover_api_key(&pool).await.unwrap().as_deref(),
        Some("settings-key")
    );
}

#[tokio::test]
async fn effective_hardcover_key_falls_back_to_env_when_unset() {
    let _env = EnvVarGuard::set("HARDCOVER_API_KEY", Some("env-key"));
    let pool = init_db("sqlite::memory:").await.unwrap();
    assert_eq!(
        effective_hardcover_api_key(&pool).await.unwrap().as_deref(),
        Some("env-key")
    );
}

#[tokio::test]
async fn effective_hardcover_key_is_none_when_neither_set() {
    let _env = EnvVarGuard::set("HARDCOVER_API_KEY", None);
    let pool = init_db("sqlite::memory:").await.unwrap();
    assert_eq!(effective_hardcover_api_key(&pool).await.unwrap(), None);
}

#[tokio::test]
async fn seed_hardcover_key_seeds_only_when_unset() {
    let _env = EnvVarGuard::set("HARDCOVER_API_KEY", Some("env-key"));
    let pool = init_db("sqlite::memory:").await.unwrap();

    // No saved value → env seeds it.
    seed_hardcover_key_from_env(&pool).await.unwrap();
    assert_eq!(
        get_hardcover_api_key(&pool).await.unwrap().as_deref(),
        Some("env-key")
    );

    // A subsequent settings save wins, and re-seeding does NOT clobber it.
    set_hardcover_api_key(&pool, Some("settings-key"))
        .await
        .unwrap();
    seed_hardcover_key_from_env(&pool).await.unwrap();
    assert_eq!(
        get_hardcover_api_key(&pool).await.unwrap().as_deref(),
        Some("settings-key")
    );
}

#[tokio::test]
async fn hardcover_key_status_reports_settings_source_masked() {
    let _env = EnvVarGuard::set("HARDCOVER_API_KEY", Some("env-key"));
    let pool = init_db("sqlite::memory:").await.unwrap();
    set_hardcover_api_key(&pool, Some("abcdefghijkl"))
        .await
        .unwrap();

    let status = hardcover_key_status(&pool).await.unwrap();
    assert!(status.configured);
    assert_eq!(status.source, "settings");
    // Long keys collapse to first4…last4 — never the raw value.
    assert_eq!(status.masked.as_deref(), Some("abcd\u{2026}ijkl"));
}

#[tokio::test]
async fn hardcover_key_status_falls_back_to_env_masked() {
    let _env = EnvVarGuard::set("HARDCOVER_API_KEY", Some("abcdefghijkl"));
    let pool = init_db("sqlite::memory:").await.unwrap();

    let status = hardcover_key_status(&pool).await.unwrap();
    assert!(status.configured);
    assert_eq!(status.source, "env");
    assert_eq!(status.masked.as_deref(), Some("abcd\u{2026}ijkl"));
}

#[tokio::test]
async fn hardcover_key_status_masks_short_keys_as_bullets() {
    let _env = EnvVarGuard::set("HARDCOVER_API_KEY", None);
    let pool = init_db("sqlite::memory:").await.unwrap();
    set_hardcover_api_key(&pool, Some("short")).await.unwrap();

    let status = hardcover_key_status(&pool).await.unwrap();
    assert!(status.configured);
    assert_eq!(status.source, "settings");
    // Keys of 8 chars or fewer never leak length beyond the fixed bullet run.
    assert_eq!(
        status.masked.as_deref(),
        Some("\u{2022}\u{2022}\u{2022}\u{2022}")
    );
}

#[tokio::test]
async fn hardcover_key_status_is_unset_when_neither_set() {
    let _env = EnvVarGuard::set("HARDCOVER_API_KEY", None);
    let pool = init_db("sqlite::memory:").await.unwrap();

    let status = hardcover_key_status(&pool).await.unwrap();
    assert!(!status.configured);
    assert_eq!(status.source, "none");
    assert_eq!(status.masked, None);
}

#[tokio::test]
async fn google_books_key_set_get_and_clear_roundtrips() {
    let _env = EnvVarGuard::set("GOOGLE_BOOKS_API_KEY", None);
    let pool = init_db("sqlite::memory:").await.unwrap();
    assert_eq!(get_google_books_api_key(&pool).await.unwrap(), None);

    set_google_books_api_key(&pool, Some("  AIzaSecret  "))
        .await
        .unwrap();
    assert_eq!(
        get_google_books_api_key(&pool).await.unwrap().as_deref(),
        Some("AIzaSecret")
    );

    // Blank clears (treated as None).
    set_google_books_api_key(&pool, Some("   ")).await.unwrap();
    assert_eq!(get_google_books_api_key(&pool).await.unwrap(), None);
}

#[tokio::test]
async fn set_google_books_api_key_accepts_value_at_max_len() {
    let _env = EnvVarGuard::set("GOOGLE_BOOKS_API_KEY", None);
    let pool = init_db("sqlite::memory:").await.unwrap();
    let at_limit = "a".repeat(GOOGLE_BOOKS_API_KEY_MAX_LEN);
    set_google_books_api_key(&pool, Some(&at_limit))
        .await
        .unwrap();
    assert_eq!(
        get_google_books_api_key(&pool).await.unwrap().as_deref(),
        Some(at_limit.as_str())
    );
}

#[tokio::test]
async fn set_google_books_api_key_rejects_value_over_max_len() {
    let _env = EnvVarGuard::set("GOOGLE_BOOKS_API_KEY", None);
    let pool = init_db("sqlite::memory:").await.unwrap();
    let over_limit = "a".repeat(GOOGLE_BOOKS_API_KEY_MAX_LEN + 1);
    let err = set_google_books_api_key(&pool, Some(&over_limit))
        .await
        .expect_err("over-limit value should be rejected");
    match &err {
        SettingsError::Validation(msg) => assert!(
            msg.contains(&GOOGLE_BOOKS_API_KEY_MAX_LEN.to_string()),
            "error message should name the cap: {msg}"
        ),
        other => panic!("expected SettingsError::Validation, got {other:?}"),
    }
    // Validation must short-circuit before the KV write.
    assert_eq!(get_google_books_api_key(&pool).await.unwrap(), None);
}

#[tokio::test]
async fn effective_google_books_key_prefers_saved_over_env() {
    let _env = EnvVarGuard::set("GOOGLE_BOOKS_API_KEY", Some("env-key"));
    let pool = init_db("sqlite::memory:").await.unwrap();
    set_google_books_api_key(&pool, Some("settings-key"))
        .await
        .unwrap();
    assert_eq!(
        effective_google_books_api_key(&pool)
            .await
            .unwrap()
            .as_deref(),
        Some("settings-key")
    );
}

#[tokio::test]
async fn effective_google_books_key_falls_back_to_env_when_unset() {
    let _env = EnvVarGuard::set("GOOGLE_BOOKS_API_KEY", Some("env-key"));
    let pool = init_db("sqlite::memory:").await.unwrap();
    assert_eq!(
        effective_google_books_api_key(&pool)
            .await
            .unwrap()
            .as_deref(),
        Some("env-key")
    );
}

#[tokio::test]
async fn effective_google_books_key_is_none_when_neither_set() {
    let _env = EnvVarGuard::set("GOOGLE_BOOKS_API_KEY", None);
    let pool = init_db("sqlite::memory:").await.unwrap();
    assert_eq!(effective_google_books_api_key(&pool).await.unwrap(), None);
}

#[tokio::test]
async fn seed_google_books_key_seeds_only_when_unset() {
    let _env = EnvVarGuard::set("GOOGLE_BOOKS_API_KEY", Some("env-key"));
    let pool = init_db("sqlite::memory:").await.unwrap();

    // No saved value → env seeds it.
    seed_google_books_key_from_env(&pool).await.unwrap();
    assert_eq!(
        get_google_books_api_key(&pool).await.unwrap().as_deref(),
        Some("env-key")
    );

    // A subsequent settings save wins, and re-seeding does NOT clobber it.
    set_google_books_api_key(&pool, Some("settings-key"))
        .await
        .unwrap();
    seed_google_books_key_from_env(&pool).await.unwrap();
    assert_eq!(
        get_google_books_api_key(&pool).await.unwrap().as_deref(),
        Some("settings-key")
    );
}

#[tokio::test]
async fn google_books_key_status_reports_settings_source_masked() {
    let _env = EnvVarGuard::set("GOOGLE_BOOKS_API_KEY", Some("env-key"));
    let pool = init_db("sqlite::memory:").await.unwrap();
    set_google_books_api_key(&pool, Some("abcdefghijkl"))
        .await
        .unwrap();

    let status = google_books_key_status(&pool).await.unwrap();
    assert!(status.configured);
    assert_eq!(status.source, "settings");
    // Long keys collapse to first4…last4 — never the raw value.
    assert_eq!(status.masked.as_deref(), Some("abcd\u{2026}ijkl"));
}

#[tokio::test]
async fn google_books_key_status_falls_back_to_env_masked() {
    let _env = EnvVarGuard::set("GOOGLE_BOOKS_API_KEY", Some("abcdefghijkl"));
    let pool = init_db("sqlite::memory:").await.unwrap();

    let status = google_books_key_status(&pool).await.unwrap();
    assert!(status.configured);
    assert_eq!(status.source, "env");
    assert_eq!(status.masked.as_deref(), Some("abcd\u{2026}ijkl"));
}

#[tokio::test]
async fn google_books_key_status_masks_short_keys_as_bullets() {
    let _env = EnvVarGuard::set("GOOGLE_BOOKS_API_KEY", None);
    let pool = init_db("sqlite::memory:").await.unwrap();
    set_google_books_api_key(&pool, Some("short"))
        .await
        .unwrap();

    let status = google_books_key_status(&pool).await.unwrap();
    assert!(status.configured);
    assert_eq!(status.source, "settings");
    // Keys of 8 chars or fewer never leak length beyond the fixed bullet run.
    assert_eq!(
        status.masked.as_deref(),
        Some("\u{2022}\u{2022}\u{2022}\u{2022}")
    );
}

#[tokio::test]
async fn google_books_key_status_is_unset_when_neither_set() {
    let _env = EnvVarGuard::set("GOOGLE_BOOKS_API_KEY", None);
    let pool = init_db("sqlite::memory:").await.unwrap();

    let status = google_books_key_status(&pool).await.unwrap();
    assert!(!status.configured);
    assert_eq!(status.source, "none");
    assert_eq!(status.masked, None);
}

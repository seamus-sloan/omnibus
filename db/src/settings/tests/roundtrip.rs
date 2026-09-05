//! `get_settings` / `set_settings`: the empty database, the round trip
//! including `scan_interval_hours` (cleared, unparseable), updating and
//! clearing values, env-var seeding, the MCP toggle, and the DB-failure
//! paths.

use super::super::*;
use crate::pool::init_db;
use crate::test_support::EnvVarGuard;

// Tests
#[tokio::test]
async fn get_settings_returns_none_for_empty_db() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let settings = get_settings(&pool).await.unwrap();
    assert_eq!(settings.ebook_library_path, None);
    assert_eq!(settings.audiobook_library_path, None);
}

#[tokio::test]
async fn set_and_get_settings_roundtrips() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let input = Settings {
        ebook_library_path: Some("/books/ebooks".into()),
        audiobook_library_path: Some("/books/audio".into()),
        scan_interval_hours: None,
    };
    set_settings(&pool, &input).await.unwrap();
    assert_eq!(get_settings(&pool).await.unwrap(), input);
}

#[tokio::test]
async fn set_and_get_settings_roundtrips_scan_interval_hours() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let input = Settings {
        ebook_library_path: None,
        audiobook_library_path: None,
        scan_interval_hours: Some(6),
    };
    set_settings(&pool, &input).await.unwrap();
    assert_eq!(get_settings(&pool).await.unwrap(), input);
}

#[tokio::test]
async fn set_settings_clears_scan_interval_hours_when_set_to_none() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    set_settings(
        &pool,
        &Settings {
            ebook_library_path: None,
            audiobook_library_path: None,
            scan_interval_hours: Some(12),
        },
    )
    .await
    .unwrap();
    set_settings(
        &pool,
        &Settings {
            ebook_library_path: None,
            audiobook_library_path: None,
            scan_interval_hours: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(get_settings(&pool).await.unwrap().scan_interval_hours, None);
}

#[tokio::test]
async fn get_settings_treats_an_unparseable_scan_interval_row_as_unset() {
    // A hand-edited or corrupted `settings` row must not error the whole
    // read — periodic scanning just stays disabled.
    let pool = init_db("sqlite::memory:").await.unwrap();
    sqlx::query("INSERT INTO settings (key, value) VALUES ('scan_interval_hours', 'not-a-number')")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(get_settings(&pool).await.unwrap().scan_interval_hours, None);
}

#[tokio::test]
async fn set_settings_updates_existing_values() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    set_settings(
        &pool,
        &Settings {
            ebook_library_path: Some("/old".into()),
            audiobook_library_path: None,
            scan_interval_hours: None,
        },
    )
    .await
    .unwrap();
    set_settings(
        &pool,
        &Settings {
            ebook_library_path: Some("/new".into()),
            audiobook_library_path: Some("/audio".into()),
            scan_interval_hours: None,
        },
    )
    .await
    .unwrap();
    let result = get_settings(&pool).await.unwrap();
    assert_eq!(result.ebook_library_path, Some("/new".into()));
    assert_eq!(result.audiobook_library_path, Some("/audio".into()));
}

#[tokio::test]
async fn set_settings_none_clears_existing_value() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    set_settings(
        &pool,
        &Settings {
            ebook_library_path: Some("/books".into()),
            audiobook_library_path: Some("/audio".into()),
            scan_interval_hours: None,
        },
    )
    .await
    .unwrap();
    set_settings(
        &pool,
        &Settings {
            ebook_library_path: None,
            audiobook_library_path: None,
            scan_interval_hours: None,
        },
    )
    .await
    .unwrap();
    let result = get_settings(&pool).await.unwrap();
    assert_eq!(result.ebook_library_path, None);
    assert_eq!(result.audiobook_library_path, None);
}

#[tokio::test]
async fn seed_settings_from_env_writes_env_vars_to_db() {
    // The guards serialize the process-global env mutation against the
    // other `seed_settings_from_env` test and restore prior values on
    // drop, so this test can't leak `EBOOK_LIBRARY_PATH` into the rest of
    // the run.
    let _env = EnvVarGuard::set("EBOOK_LIBRARY_PATH", Some("/env/books"))
        .also_set("AUDIOBOOK_LIBRARY_PATH", Some("/env/audio"));
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_settings_from_env(&pool).await.unwrap();
    let result = get_settings(&pool).await.unwrap();
    assert_eq!(result.ebook_library_path, Some("/env/books".into()));
    assert_eq!(result.audiobook_library_path, Some("/env/audio".into()));
}

#[tokio::test]
async fn seed_settings_from_env_is_noop_when_vars_unset() {
    // Guard restores prior values on drop. Both vars are removed to
    // establish the "unset" precondition this test exercises.
    let _env =
        EnvVarGuard::set("EBOOK_LIBRARY_PATH", None).also_set("AUDIOBOOK_LIBRARY_PATH", None);
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_settings_from_env(&pool).await.unwrap();
    let result = get_settings(&pool).await.unwrap();
    assert_eq!(result.ebook_library_path, None);
    assert_eq!(result.audiobook_library_path, None);
}

#[tokio::test]
async fn get_settings_propagates_db_error_when_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    pool.close().await;
    let err = get_settings(&pool).await.unwrap_err();
    assert!(matches!(err, SettingsError::Db(_)));
}

#[tokio::test]
async fn mcp_enabled_defaults_off_and_round_trips() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    // Default OFF: a fresh install exposes no MCP surface.
    assert!(!mcp_enabled(&pool).await.unwrap());

    set_mcp_enabled(&pool, true).await.unwrap();
    assert!(mcp_enabled(&pool).await.unwrap());

    set_mcp_enabled(&pool, false).await.unwrap();
    assert!(!mcp_enabled(&pool).await.unwrap());
}

#[tokio::test]
async fn mcp_enabled_propagates_db_error_when_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    pool.close().await;
    assert!(mcp_enabled(&pool).await.is_err());
    assert!(set_mcp_enabled(&pool, true).await.is_err());
}

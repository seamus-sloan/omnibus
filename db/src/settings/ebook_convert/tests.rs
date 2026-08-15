use crate::pool::init_db;
use crate::test_support::EnvVarGuard;

use super::*;

#[tokio::test]
async fn get_ebook_convert_path_returns_none_when_nothing_is_saved() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    assert_eq!(get_ebook_convert_path(&pool).await.unwrap(), None);
}

#[tokio::test]
async fn set_and_get_ebook_convert_path_roundtrips_a_trimmed_value() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    set_ebook_convert_path(&pool, Some("  /opt/calibre/ebook-convert  "))
        .await
        .unwrap();
    assert_eq!(
        get_ebook_convert_path(&pool).await.unwrap().as_deref(),
        Some("/opt/calibre/ebook-convert")
    );
}

#[tokio::test]
async fn set_ebook_convert_path_clears_the_row_when_given_none() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    set_ebook_convert_path(&pool, Some("/opt/calibre/ebook-convert"))
        .await
        .unwrap();
    set_ebook_convert_path(&pool, None).await.unwrap();
    assert_eq!(get_ebook_convert_path(&pool).await.unwrap(), None);
}

#[tokio::test]
async fn set_ebook_convert_path_clears_the_row_when_given_a_blank_value() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    set_ebook_convert_path(&pool, Some("/opt/calibre/ebook-convert"))
        .await
        .unwrap();
    set_ebook_convert_path(&pool, Some("   ")).await.unwrap();
    assert_eq!(get_ebook_convert_path(&pool).await.unwrap(), None);
}

#[tokio::test]
async fn set_ebook_convert_path_rejects_a_path_over_the_length_cap() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let too_long = "x".repeat(PATH_MAX_LEN + 1);
    let err = set_ebook_convert_path(&pool, Some(&too_long))
        .await
        .unwrap_err();
    assert!(matches!(err, SettingsError::Validation(_)));
    assert_eq!(get_ebook_convert_path(&pool).await.unwrap(), None);
}

#[tokio::test]
async fn set_ebook_convert_path_propagates_a_db_error_when_the_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    pool.close().await;
    let err = set_ebook_convert_path(&pool, Some("/opt/calibre/ebook-convert"))
        .await
        .unwrap_err();
    assert!(matches!(err, SettingsError::Db(_)));
}

#[tokio::test]
async fn effective_ebook_convert_path_falls_back_to_the_bare_name_when_nothing_is_configured() {
    let _guard = EnvVarGuard::set("OMNIBUS_EBOOK_CONVERT_PATH", None);
    let pool = init_db("sqlite::memory:").await.unwrap();
    let (path, source) = effective_ebook_convert_path(&pool).await.unwrap();
    assert_eq!(path, DEFAULT_EBOOK_CONVERT_BIN);
    assert_eq!(source, "path");
}

#[tokio::test]
async fn effective_ebook_convert_path_uses_the_env_var_when_no_override_is_saved() {
    let _guard = EnvVarGuard::set(
        "OMNIBUS_EBOOK_CONVERT_PATH",
        Some("/from/env/ebook-convert"),
    );
    let pool = init_db("sqlite::memory:").await.unwrap();
    let (path, source) = effective_ebook_convert_path(&pool).await.unwrap();
    assert_eq!(path, "/from/env/ebook-convert");
    assert_eq!(source, "env");
}

#[tokio::test]
async fn effective_ebook_convert_path_prefers_the_saved_override_over_the_env_var() {
    let _guard = EnvVarGuard::set(
        "OMNIBUS_EBOOK_CONVERT_PATH",
        Some("/from/env/ebook-convert"),
    );
    let pool = init_db("sqlite::memory:").await.unwrap();
    set_ebook_convert_path(&pool, Some("/from/settings/ebook-convert"))
        .await
        .unwrap();
    let (path, source) = effective_ebook_convert_path(&pool).await.unwrap();
    assert_eq!(path, "/from/settings/ebook-convert");
    assert_eq!(source, "settings");
}

#[tokio::test]
async fn seed_ebook_convert_path_from_env_writes_the_env_value_when_nothing_is_saved() {
    let _guard = EnvVarGuard::set(
        "OMNIBUS_EBOOK_CONVERT_PATH",
        Some("/from/env/ebook-convert"),
    );
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_ebook_convert_path_from_env(&pool).await.unwrap();
    assert_eq!(
        get_ebook_convert_path(&pool).await.unwrap().as_deref(),
        Some("/from/env/ebook-convert")
    );
}

#[tokio::test]
async fn seed_ebook_convert_path_from_env_leaves_a_saved_override_intact() {
    let _guard = EnvVarGuard::set(
        "OMNIBUS_EBOOK_CONVERT_PATH",
        Some("/from/env/ebook-convert"),
    );
    let pool = init_db("sqlite::memory:").await.unwrap();
    set_ebook_convert_path(&pool, Some("/from/settings/ebook-convert"))
        .await
        .unwrap();
    seed_ebook_convert_path_from_env(&pool).await.unwrap();
    assert_eq!(
        get_ebook_convert_path(&pool).await.unwrap().as_deref(),
        Some("/from/settings/ebook-convert")
    );
}

#[tokio::test]
async fn seed_ebook_convert_path_from_env_is_a_no_op_when_the_env_var_is_unset() {
    let _guard = EnvVarGuard::set("OMNIBUS_EBOOK_CONVERT_PATH", None);
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_ebook_convert_path_from_env(&pool).await.unwrap();
    assert_eq!(get_ebook_convert_path(&pool).await.unwrap(), None);
}

#[tokio::test]
async fn ebook_convert_status_reports_unavailable_for_a_missing_binary() {
    let _guard = EnvVarGuard::set("OMNIBUS_EBOOK_CONVERT_PATH", None);
    let pool = init_db("sqlite::memory:").await.unwrap();
    set_ebook_convert_path(&pool, Some("/nonexistent/omnibus-ebook-convert-probe"))
        .await
        .unwrap();
    let status = ebook_convert_status(&pool).await.unwrap();
    assert!(!status.available);
    assert_eq!(status.path, "/nonexistent/omnibus-ebook-convert-probe");
    assert_eq!(status.source, "settings");
}

#[tokio::test]
async fn ebook_convert_status_reports_available_for_a_runnable_binary() {
    let _guard = EnvVarGuard::set("OMNIBUS_EBOOK_CONVERT_PATH", None);
    let pool = init_db("sqlite::memory:").await.unwrap();
    // `env --version` exits 0 everywhere the server runs, standing in for a
    // real Calibre install.
    set_ebook_convert_path(&pool, Some("env")).await.unwrap();
    let status = ebook_convert_status(&pool).await.unwrap();
    assert!(status.available);
    assert_eq!(status.source, "settings");
}

#[tokio::test]
async fn ebook_convert_status_propagates_a_db_error_when_the_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    pool.close().await;
    let err = ebook_convert_status(&pool).await.unwrap_err();
    assert!(matches!(err, SettingsError::Db(_)));
}

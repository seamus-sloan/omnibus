//! The Send-to-Kindle SMTP config: round trip, the host+from requirement,
//! a `None` password preserving the saved one, from-address and port
//! validation, clearing, the masked status, and the settings-over-env
//! resolution and seeding.

use super::super::*;
use crate::pool::init_db;
use crate::test_support::EnvVarGuard;

// SMTP config (F4.3)
fn smtp_update() -> SmtpConfigUpdate {
    SmtpConfigUpdate {
        host: "smtp.example.com".into(),
        port: 587,
        username: "postmaster".into(),
        from_email: "library@example.com".into(),
        security: SmtpSecurity::Starttls,
        password: Some("s3cret-pass".into()),
    }
}

#[tokio::test]
async fn set_and_get_smtp_config_roundtrips() {
    let _env = EnvVarGuard::set("SMTP_HOST", None).also_set("SMTP_FROM_EMAIL", None);
    let pool = init_db("sqlite::memory:").await.unwrap();
    set_smtp_config(&pool, &smtp_update()).await.unwrap();
    let c = get_smtp_config(&pool).await.unwrap().unwrap();
    assert_eq!(c.host, "smtp.example.com");
    assert_eq!(c.port, 587);
    assert_eq!(c.username, "postmaster");
    assert_eq!(c.password, "s3cret-pass");
    assert_eq!(c.from_email, "library@example.com");
    assert_eq!(c.security, SmtpSecurity::Starttls);
}

#[tokio::test]
async fn get_smtp_config_returns_none_without_host_and_from() {
    let _env = EnvVarGuard::set("SMTP_HOST", None).also_set("SMTP_FROM_EMAIL", None);
    let pool = init_db("sqlite::memory:").await.unwrap();
    assert!(get_smtp_config(&pool).await.unwrap().is_none());
}

#[tokio::test]
async fn set_smtp_config_none_password_preserves_existing() {
    let _env = EnvVarGuard::set("SMTP_HOST", None).also_set("SMTP_FROM_EMAIL", None);
    let pool = init_db("sqlite::memory:").await.unwrap();
    set_smtp_config(&pool, &smtp_update()).await.unwrap();
    let mut update = smtp_update();
    update.host = "smtp2.example.com".into();
    update.password = None; // leave the stored password untouched
    set_smtp_config(&pool, &update).await.unwrap();
    let c = get_smtp_config(&pool).await.unwrap().unwrap();
    assert_eq!(c.host, "smtp2.example.com");
    assert_eq!(c.password, "s3cret-pass");
}

#[tokio::test]
async fn set_smtp_config_rejects_invalid_from_email() {
    let _env = EnvVarGuard::set("SMTP_HOST", None).also_set("SMTP_FROM_EMAIL", None);
    let pool = init_db("sqlite::memory:").await.unwrap();
    let mut update = smtp_update();
    update.from_email = "not-an-email".into();
    let err = set_smtp_config(&pool, &update).await.unwrap_err();
    assert!(matches!(err, SettingsError::Validation(_)));
}

#[tokio::test]
async fn set_smtp_config_rejects_port_zero() {
    let _env = EnvVarGuard::set("SMTP_HOST", None).also_set("SMTP_FROM_EMAIL", None);
    let pool = init_db("sqlite::memory:").await.unwrap();
    let mut update = smtp_update();
    update.port = 0;
    let err = set_smtp_config(&pool, &update).await.unwrap_err();
    assert!(matches!(err, SettingsError::Validation(_)));
}

#[tokio::test]
async fn clear_smtp_config_removes_all_rows() {
    let _env = EnvVarGuard::set("SMTP_HOST", None).also_set("SMTP_FROM_EMAIL", None);
    let pool = init_db("sqlite::memory:").await.unwrap();
    set_smtp_config(&pool, &smtp_update()).await.unwrap();
    clear_smtp_config(&pool).await.unwrap();
    assert!(get_smtp_config(&pool).await.unwrap().is_none());
}

#[tokio::test]
async fn smtp_status_masks_password_and_reports_settings_source() {
    let _env = EnvVarGuard::set("SMTP_HOST", None).also_set("SMTP_FROM_EMAIL", None);
    let pool = init_db("sqlite::memory:").await.unwrap();
    set_smtp_config(&pool, &smtp_update()).await.unwrap();
    let status = smtp_status(&pool).await.unwrap();
    assert!(status.configured);
    assert_eq!(status.source, "settings");
    assert_eq!(status.host.as_deref(), Some("smtp.example.com"));
    let masked = status.password_masked.unwrap();
    assert!(!masked.contains("s3cret-pass"));
}

#[tokio::test]
async fn effective_smtp_config_falls_back_to_env_when_unset() {
    let _env = EnvVarGuard::set("SMTP_HOST", Some("env-smtp.example.com"))
        .also_set("SMTP_FROM_EMAIL", Some("env-from@example.com"));
    let pool = init_db("sqlite::memory:").await.unwrap();
    let c = effective_smtp_config(&pool).await.unwrap().unwrap();
    assert_eq!(c.host, "env-smtp.example.com");
    assert_eq!(c.from_email, "env-from@example.com");
}

#[tokio::test]
async fn effective_smtp_config_prefers_saved_over_env() {
    let _env = EnvVarGuard::set("SMTP_HOST", Some("env-smtp.example.com"))
        .also_set("SMTP_FROM_EMAIL", Some("env-from@example.com"));
    let pool = init_db("sqlite::memory:").await.unwrap();
    set_smtp_config(&pool, &smtp_update()).await.unwrap();
    let c = effective_smtp_config(&pool).await.unwrap().unwrap();
    assert_eq!(c.host, "smtp.example.com");
}

#[tokio::test]
async fn seed_smtp_from_env_only_seeds_when_unset() {
    let _env = EnvVarGuard::set("SMTP_HOST", Some("env-smtp.example.com"))
        .also_set("SMTP_FROM_EMAIL", Some("env-from@example.com"));
    let pool = init_db("sqlite::memory:").await.unwrap();
    set_smtp_config(&pool, &smtp_update()).await.unwrap();
    seed_smtp_from_env(&pool).await.unwrap();
    // Settings wins — the env seed is a no-op because a host is already saved.
    let c = get_smtp_config(&pool).await.unwrap().unwrap();
    assert_eq!(c.host, "smtp.example.com");
}

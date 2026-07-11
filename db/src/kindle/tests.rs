//! Unit tests for the Send-to-Kindle module: the pre-network error paths
//! (`NotConfigured` / `NoEpub`) and the pure MIME-message builder. The actual
//! SMTP transport isn't exercised — that needs a live relay and belongs to the
//! end-to-end suite.

use super::*;
use crate::pool::init_db;
use crate::settings::{set_smtp_config, SmtpConfigUpdate, SmtpSecurity};
use crate::test_support::EnvVarGuard;

/// Set a usable SMTP config so the send path gets past the `NotConfigured`
/// gate and reaches EPUB resolution.
async fn seed_smtp(pool: &sqlx::SqlitePool) {
    set_smtp_config(
        pool,
        &SmtpConfigUpdate {
            host: "smtp.example.com".into(),
            port: 587,
            username: "user".into(),
            from_email: "library@example.com".into(),
            security: SmtpSecurity::Starttls,
            password: Some("secret".into()),
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn send_returns_not_configured_when_no_smtp_config() {
    let _env = EnvVarGuard::set("SMTP_HOST", None).also_set("SMTP_FROM_EMAIL", None);
    let pool = init_db("sqlite::memory:").await.unwrap();
    let err = send(&pool, 1, None, "reader@kindle.com").await.unwrap_err();
    assert!(matches!(err, KindleError::NotConfigured));
}

#[tokio::test]
async fn send_returns_no_epub_when_config_present_but_book_missing() {
    let _env = EnvVarGuard::set("SMTP_HOST", None).also_set("SMTP_FROM_EMAIL", None);
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_smtp(&pool).await;
    // No books seeded, so path resolution yields None → NoEpub (reached only
    // because the SMTP gate passed).
    let err = send(&pool, 999, None, "reader@kindle.com")
        .await
        .unwrap_err();
    assert!(matches!(err, KindleError::NoEpub));
}

#[tokio::test]
async fn send_returns_too_large_when_epub_exceeds_email_cap() {
    let _env = EnvVarGuard::set("SMTP_HOST", None).also_set("SMTP_FROM_EMAIL", None);
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_smtp(&pool).await;

    // Point a scan root at a temp dir holding a sparse EPUB one byte over the
    // email cap, so `send` reaches the size guard after resolving the path.
    let dir = tempfile::TempDir::new().unwrap();
    let lib = dir.path().to_str().unwrap();
    let epub = dir.path().join("big.epub");
    std::fs::File::create(&epub)
        .unwrap()
        .set_len(omnibus_shared::KINDLE_EMAIL_MAX_BYTES + 1)
        .unwrap();

    let lib_id = sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES (?, 'lib')")
        .bind(lib)
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_rowid();
    let book_id = sqlx::query(
        "INSERT INTO books (uuid, library_id, path, title) VALUES ('uuid-big', ?, '', 'Big')",
    )
    .bind(lib_id)
    .execute(&pool)
    .await
    .unwrap()
    .last_insert_rowid();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes) \
         VALUES (?, 'EPUB', 'big', 0)",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();

    let err = send(&pool, book_id, None, "reader@kindle.com")
        .await
        .unwrap_err();
    assert!(matches!(err, KindleError::TooLarge), "got {err:?}");
}

#[tokio::test]
async fn send_test_returns_not_configured_when_unset() {
    let _env = EnvVarGuard::set("SMTP_HOST", None).also_set("SMTP_FROM_EMAIL", None);
    let pool = init_db("sqlite::memory:").await.unwrap();
    let err = send_test(&pool, "admin@kindle.com").await.unwrap_err();
    assert!(matches!(err, KindleError::NotConfigured));
}

#[test]
fn build_epub_email_uses_filename_stem_as_subject_and_attaches_epub() {
    let msg = build_epub_email(
        "library@example.com",
        "reader@kindle.com",
        "The Great Gatsby.epub",
        b"epub-bytes".to_vec(),
    )
    .unwrap();
    let formatted = String::from_utf8(msg.formatted()).unwrap();
    assert!(formatted.contains("Subject: The Great Gatsby"));
    assert!(formatted.contains("application/epub+zip"));
    assert!(formatted.contains("The Great Gatsby.epub"));
    assert!(formatted.contains("From: library@example.com"));
    assert!(formatted.contains("To: reader@kindle.com"));
}

#[test]
fn build_epub_email_rejects_malformed_address() {
    let err = build_epub_email(
        "not-an-email",
        "reader@kindle.com",
        "book.epub",
        b"x".to_vec(),
    )
    .unwrap_err();
    assert!(matches!(err, KindleError::Address(_)));
}

#[test]
fn kindle_error_timeout_reports_the_overall_send_cap() {
    // `deliver` maps a `tokio::time::timeout` elapse to `Timeout` (a unit
    // variant) so a hung relay surfaces a bounded failure instead of hanging
    // the worker forever. Its rendered message must name the overall cap so an
    // admin can tell it apart from a per-command `Smtp` error.
    let err = KindleError::Timeout;
    assert_eq!(err.to_string(), "SMTP delivery timed out after 90s");
}

#[test]
fn kindle_error_build_wraps_lettre_message_error() {
    // The MIME builders (`build_epub_email` / `send_test`) surface a lettre
    // message-assembly failure through `#[from] lettre::error::Error` as
    // `KindleError::Build`. Feeding a real lettre error through the `From`
    // bridge asserts that mapping (and its `{0}` message) without needing a
    // live SMTP relay.
    let src = lettre::error::Error::MissingFrom;
    let err: KindleError = src.into();
    assert!(matches!(err, KindleError::Build(_)), "got {err:?}");
    assert!(
        err.to_string().starts_with("failed to build the email"),
        "got {err}"
    );
}

#[tokio::test]
async fn send_propagates_db_error_when_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    pool.close().await;
    // `send` reads the SMTP config via `crate::settings::SettingsError`
    // first, so a closed pool surfaces as the wrapped `Settings` variant.
    let err = send(&pool, 1, None, "reader@example.com")
        .await
        .unwrap_err();
    assert!(matches!(err, KindleError::Settings(_)));
}

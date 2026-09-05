//! Per-account preferences: the Kindle email (validated), the hidden
//! formats list (normalized, validated, capped, cleared) and the
//! book-detail scroll-stops flag, each read back through
//! `get_user_by_id`.

use super::super::*;
use crate::auth::test_support::pool;

#[tokio::test]
async fn set_kindle_email_roundtrips_and_clears() {
    let p = pool().await;
    let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    assert_eq!(u.kindle_email, None);

    set_kindle_email(&p, u.id, Some("alice@kindle.com"))
        .await
        .unwrap();
    assert_eq!(
        get_kindle_email(&p, u.id).await.unwrap().as_deref(),
        Some("alice@kindle.com")
    );
    let reloaded = get_user_by_id(&p, u.id).await.unwrap().unwrap();
    assert_eq!(reloaded.kindle_email.as_deref(), Some("alice@kindle.com"));

    // Clearing with None wipes it.
    set_kindle_email(&p, u.id, None).await.unwrap();
    assert_eq!(get_kindle_email(&p, u.id).await.unwrap(), None);
}

#[tokio::test]
async fn set_kindle_email_rejects_malformed_address() {
    let p = pool().await;
    let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    let err = set_kindle_email(&p, u.id, Some("nope")).await.unwrap_err();
    assert!(matches!(err, AuthError::Validation(_)));
}

#[tokio::test]
async fn set_hidden_formats_normalizes_lowercases_dedupes_and_sorts() {
    let p = pool().await;
    let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    assert!(u.hidden_formats.is_empty());

    let stored = set_hidden_formats(
        &p,
        u.id,
        &["CBZ".into(), " m4b ".into(), "cbz".into(), "".into()],
    )
    .await
    .unwrap();
    assert_eq!(stored, vec!["cbz".to_string(), "m4b".to_string()]);
    assert_eq!(get_hidden_formats(&p, u.id).await.unwrap(), stored);
}

#[tokio::test]
async fn set_hidden_formats_rejects_malformed_token_with_validation_error() {
    let p = pool().await;
    let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    let err = set_hidden_formats(&p, u.id, &["not a format!".into()])
        .await
        .unwrap_err();
    assert!(matches!(err, AuthError::Validation(_)));
    // The failed write must not have landed.
    assert!(get_hidden_formats(&p, u.id).await.unwrap().is_empty());
}

#[tokio::test]
async fn set_hidden_formats_rejects_oversized_list_with_validation_error() {
    let p = pool().await;
    let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    let too_many: Vec<String> = (0..=HIDDEN_FORMATS_MAX).map(|i| format!("f{i}")).collect();
    let err = set_hidden_formats(&p, u.id, &too_many).await.unwrap_err();
    assert!(matches!(err, AuthError::Validation(_)));
}

#[tokio::test]
async fn set_hidden_formats_with_empty_list_clears_the_column() {
    let p = pool().await;
    let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    set_hidden_formats(&p, u.id, &["cbz".into()]).await.unwrap();

    set_hidden_formats(&p, u.id, &[]).await.unwrap();
    assert!(get_hidden_formats(&p, u.id).await.unwrap().is_empty());
    let raw: Option<Option<String>> =
        sqlx::query_scalar("SELECT hidden_formats FROM users WHERE id = ?")
            .bind(u.id)
            .fetch_optional(&p)
            .await
            .unwrap();
    assert_eq!(raw, Some(None), "clearing stores NULL, not an empty string");
}

#[tokio::test]
async fn get_user_by_id_carries_parsed_hidden_formats() {
    let p = pool().await;
    let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    set_hidden_formats(&p, u.id, &["cbz".into(), "pdf".into()])
        .await
        .unwrap();
    let reloaded = get_user_by_id(&p, u.id).await.unwrap().unwrap();
    assert_eq!(
        reloaded.hidden_formats,
        vec!["cbz".to_string(), "pdf".to_string()]
    );
}

#[tokio::test]
async fn set_book_detail_scroll_stops_round_trips_both_directions() {
    let p = pool().await;
    let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    // A fresh account reads the off default rather than a NULL.
    assert!(!u.book_detail_scroll_stops);
    assert!(!get_book_detail_scroll_stops(&p, u.id).await.unwrap());

    set_book_detail_scroll_stops(&p, u.id, true).await.unwrap();
    assert!(get_book_detail_scroll_stops(&p, u.id).await.unwrap());

    set_book_detail_scroll_stops(&p, u.id, false).await.unwrap();
    assert!(!get_book_detail_scroll_stops(&p, u.id).await.unwrap());
}

#[tokio::test]
async fn get_user_by_id_carries_the_book_detail_scroll_stops_flag() {
    let p = pool().await;
    let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    set_book_detail_scroll_stops(&p, u.id, true).await.unwrap();

    let reloaded = get_user_by_id(&p, u.id).await.unwrap().unwrap();
    assert!(reloaded.book_detail_scroll_stops);
}

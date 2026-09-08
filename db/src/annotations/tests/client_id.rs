//! The client-minted `client_id` handle: create is idempotent on it,
//! scoped per user, resolvable only to the owner's row, optional (no
//! handle still allows duplicates), and its lookup's DB-failure path.

use super::super::*;
use super::{seed, seed_user};
use crate::init_db;

#[tokio::test]
async fn create_highlight_is_idempotent_on_client_id() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    let input = CreateHighlight {
        client_id: Some("3f1b0c9e-0000-4000-8000-000000000001".into()),
        book_uuid: uuid.clone(),
        epub_cfi_range: "epubcfi(/6/4!/4/2)".into(),
        color: HighlightColor::Green,
        text: Some("passage".into()),
    };

    let first = create_highlight(&pool, user, &input).await.unwrap();
    // The outbox replaying a create whose response never made it back must
    // resolve to the same row, not a second highlight on the same passage.
    let second = create_highlight(&pool, user, &input).await.unwrap();

    assert_eq!(first.id, second.id);
    assert_eq!(
        first.client_id.as_deref(),
        Some(input.client_id.as_deref().unwrap())
    );
    let all = list_highlights(&pool, user, &uuid).await.unwrap();
    assert_eq!(all.len(), 1, "replayed create must not duplicate");
}

#[tokio::test]
async fn create_highlight_scopes_client_id_per_user() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let alice = seed_user(&pool, "alice").await;
    let bob = seed_user(&pool, "bob").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    let input = CreateHighlight {
        client_id: Some("shared-handle".into()),
        book_uuid: uuid.clone(),
        epub_cfi_range: "epubcfi(/6/4!/4/2)".into(),
        color: HighlightColor::Amber,
        text: None,
    };

    let a = create_highlight(&pool, alice, &input).await.unwrap();
    let b = create_highlight(&pool, bob, &input).await.unwrap();

    assert_ne!(
        a.id, b.id,
        "one account's handle must not claim another's row"
    );
}

#[tokio::test]
async fn highlight_id_for_client_id_resolves_only_the_owners_row() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let alice = seed_user(&pool, "alice").await;
    let bob = seed_user(&pool, "bob").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    let created = create_highlight(
        &pool,
        alice,
        &CreateHighlight {
            client_id: Some("alice-handle".into()),
            book_uuid: uuid.clone(),
            epub_cfi_range: "epubcfi(/6/4!/4/2)".into(),
            color: HighlightColor::Rose,
            text: None,
        },
    )
    .await
    .unwrap();

    let mine = highlight_id_for_client_id(&pool, alice, "alice-handle")
        .await
        .unwrap();
    let theirs = highlight_id_for_client_id(&pool, bob, "alice-handle")
        .await
        .unwrap();
    let missing = highlight_id_for_client_id(&pool, alice, "no-such-handle")
        .await
        .unwrap();

    assert_eq!(mine, Some(created.id));
    assert_eq!(theirs, None);
    assert_eq!(missing, None);
}

#[tokio::test]
async fn create_highlight_without_client_id_still_allows_duplicates() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    let input = CreateHighlight {
        client_id: None,
        book_uuid: uuid.clone(),
        epub_cfi_range: "epubcfi(/6/4!/4/2)".into(),
        color: HighlightColor::Amber,
        text: None,
    };

    // The partial-unique index must leave NULL client_ids unconstrained —
    // two deliberate highlights on the same passage are legal.
    create_highlight(&pool, user, &input).await.unwrap();
    create_highlight(&pool, user, &input).await.unwrap();

    assert_eq!(list_highlights(&pool, user, &uuid).await.unwrap().len(), 2);
}

#[tokio::test]
async fn highlight_id_for_client_id_propagates_db_error_when_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    pool.close().await;
    let err = highlight_id_for_client_id(&pool, 1, "handle")
        .await
        .unwrap_err();
    assert!(matches!(err, HighlightError::Sqlx(_)));
}

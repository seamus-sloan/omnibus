//! Unit tests for the `highlights` CRUD module — create round-trip,
//! BookNotFound / NotFound variants, list isolation, color + note
//! updates, and delete behaviour.

use super::*;
use crate::{init_db, replace_books};
use omnibus_shared::EbookMetadata;

async fn seed(pool: &SqlitePool, library: &str, title: &str) -> (i64, String) {
    replace_books(
        pool,
        library,
        vec![crate::ebook::IndexedBook {
            metadata: EbookMetadata {
                filename: format!("{title}.epub").to_lowercase(),
                title: Some(title.to_string()),
                ..Default::default()
            },
            cover: None,
            mtime_epoch: 0,
            size_bytes: 0,
        }],
    )
    .await
    .expect("seed book");
    let books = crate::list_books(pool, library).await.unwrap();
    let book = books
        .into_iter()
        .find(|b| b.title.as_deref() == Some(title))
        .unwrap();
    (book.id, book.unique_identifier.clone().unwrap())
}

async fn seed_user(pool: &SqlitePool, name: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO users (username, password_hash, is_admin, can_upload, can_edit, can_download)
         VALUES (?, '!x', 0, 0, 0, 1) RETURNING id",
    )
    .bind(name)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn create_highlight_round_trips_fields() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    let input = CreateHighlight {
        book_uuid: uuid.clone(),
        epub_cfi_range: "epubcfi(/6/4!/4/2,/1:0,/1:100)".into(),
        color: HighlightColor::Blue,
    };
    let h = create_highlight(&pool, user, &input).await.unwrap();
    assert_eq!(h.book_uuid, uuid);
    assert_eq!(h.epub_cfi_range, "epubcfi(/6/4!/4/2,/1:0,/1:100)");
    assert_eq!(h.color, HighlightColor::Blue);
    assert!(h.note.is_none());
    assert!(h.created_at > 0);
}

#[tokio::test]
async fn create_highlight_returns_book_not_found_for_unknown_uuid() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let input = CreateHighlight {
        book_uuid: "no-such-uuid".into(),
        epub_cfi_range: "epubcfi(/6/4)".into(),
        color: HighlightColor::Amber,
    };
    let err = create_highlight(&pool, user, &input).await.unwrap_err();
    assert!(matches!(err, HighlightError::BookNotFound));
}

#[tokio::test]
async fn list_highlights_returns_empty_when_none_exist() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    let list = list_highlights(&pool, user, &uuid).await.unwrap();
    assert!(list.is_empty());
}

#[tokio::test]
async fn list_highlights_isolates_by_user_and_book() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let alice = seed_user(&pool, "alice").await;
    let bob = seed_user(&pool, "bob").await;
    let (_, uuid_a) = seed(&pool, "/lib-a", "Book A").await;
    let (_, uuid_b) = seed(&pool, "/lib-b", "Book B").await;

    let input_a = CreateHighlight {
        book_uuid: uuid_a.clone(),
        epub_cfi_range: "epubcfi(/6/4)".into(),
        color: HighlightColor::Amber,
    };
    let input_b = CreateHighlight {
        book_uuid: uuid_b.clone(),
        epub_cfi_range: "epubcfi(/6/8)".into(),
        color: HighlightColor::Green,
    };
    create_highlight(&pool, alice, &input_a).await.unwrap();
    create_highlight(&pool, alice, &input_b).await.unwrap();
    create_highlight(&pool, bob, &input_a).await.unwrap();

    let alice_a = list_highlights(&pool, alice, &uuid_a).await.unwrap();
    assert_eq!(alice_a.len(), 1);
    let alice_b = list_highlights(&pool, alice, &uuid_b).await.unwrap();
    assert_eq!(alice_b.len(), 1);
    let bob_a = list_highlights(&pool, bob, &uuid_a).await.unwrap();
    assert_eq!(bob_a.len(), 1);
    let bob_b = list_highlights(&pool, bob, &uuid_b).await.unwrap();
    assert!(bob_b.is_empty());
}

#[tokio::test]
async fn update_highlight_color_changes_color() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    let h = create_highlight(
        &pool,
        user,
        &CreateHighlight {
            book_uuid: uuid.clone(),
            epub_cfi_range: "epubcfi(/6/4)".into(),
            color: HighlightColor::Amber,
        },
    )
    .await
    .unwrap();

    update_highlight_color(&pool, user, h.id, HighlightColor::Violet)
        .await
        .unwrap();
    let list = list_highlights(&pool, user, &uuid).await.unwrap();
    assert_eq!(list[0].color, HighlightColor::Violet);
}

#[tokio::test]
async fn update_highlight_color_returns_not_found_for_other_user() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let alice = seed_user(&pool, "alice").await;
    let bob = seed_user(&pool, "bob").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    let h = create_highlight(
        &pool,
        alice,
        &CreateHighlight {
            book_uuid: uuid.clone(),
            epub_cfi_range: "epubcfi(/6/4)".into(),
            color: HighlightColor::Amber,
        },
    )
    .await
    .unwrap();

    let err = update_highlight_color(&pool, bob, h.id, HighlightColor::Rose)
        .await
        .unwrap_err();
    assert!(matches!(err, HighlightError::NotFound));
}

#[tokio::test]
async fn update_highlight_note_sets_and_clears() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    let h = create_highlight(
        &pool,
        user,
        &CreateHighlight {
            book_uuid: uuid.clone(),
            epub_cfi_range: "epubcfi(/6/4)".into(),
            color: HighlightColor::Green,
        },
    )
    .await
    .unwrap();
    assert!(h.note.is_none());

    update_highlight_note(&pool, user, h.id, Some("important passage"))
        .await
        .unwrap();
    let list = list_highlights(&pool, user, &uuid).await.unwrap();
    assert_eq!(list[0].note.as_deref(), Some("important passage"));

    update_highlight_note(&pool, user, h.id, None)
        .await
        .unwrap();
    let list = list_highlights(&pool, user, &uuid).await.unwrap();
    assert!(list[0].note.is_none());
}

#[tokio::test]
async fn delete_highlight_removes_row() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    let h = create_highlight(
        &pool,
        user,
        &CreateHighlight {
            book_uuid: uuid.clone(),
            epub_cfi_range: "epubcfi(/6/4)".into(),
            color: HighlightColor::Rose,
        },
    )
    .await
    .unwrap();

    delete_highlight(&pool, user, h.id).await.unwrap();
    let list = list_highlights(&pool, user, &uuid).await.unwrap();
    assert!(list.is_empty());
}

#[tokio::test]
async fn delete_highlight_returns_not_found_for_other_user() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let alice = seed_user(&pool, "alice").await;
    let bob = seed_user(&pool, "bob").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    let h = create_highlight(
        &pool,
        alice,
        &CreateHighlight {
            book_uuid: uuid.clone(),
            epub_cfi_range: "epubcfi(/6/4)".into(),
            color: HighlightColor::Blue,
        },
    )
    .await
    .unwrap();

    let err = delete_highlight(&pool, bob, h.id).await.unwrap_err();
    assert!(matches!(err, HighlightError::NotFound));
}

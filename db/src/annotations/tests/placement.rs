//! Placement and ordering of a listed annotation: that a highlight comes
//! back naming the chapter it sits in, and that position order is the
//! reader's order through the book rather than the order they marked it.

use omnibus_shared::CreateBookmark;

use super::super::*;
use super::{seed, seed_epub_structure, seed_user};
use crate::anchor::AnnotationOrder;
use crate::bookmarks::{create_bookmark, list_bookmarks};
use crate::init_db;

/// A point CFI in spine document `step`. The step is `2 * (index + 1)`, the
/// even-numbered child the spine addresses.
fn cfi_in_spine(spine_index: i64) -> String {
    format!("epubcfi(/6/{}!/4/2/1:0)", (spine_index + 1) * 2)
}

async fn highlight_at(pool: &SqlitePool, user: i64, uuid: &str, spine_index: i64) {
    create_highlight(
        pool,
        user,
        &CreateHighlight {
            book_uuid: uuid.to_string(),
            epub_cfi_range: cfi_in_spine(spine_index),
            color: HighlightColor::Amber,
            text: Some(format!("passage in {spine_index}")),
            client_id: Some(format!("h-{spine_index}")),
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn list_highlights_names_the_chapter_each_anchor_sits_in() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (book_id, uuid) = seed(&pool, "/lib", "Book A").await;
    seed_epub_structure(&pool, book_id).await;

    highlight_at(&pool, user, &uuid, 2).await;

    let list = list_highlights(&pool, user, &uuid, AnnotationOrder::Position)
        .await
        .unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].spine_index, Some(2));
    assert_eq!(
        list[0].chapter_title.as_deref(),
        Some("Two"),
        "the TOC entry owning spine 2, not the one before it"
    );
    assert_eq!(
        list[0].percent_through_book,
        Some(50.0),
        "spine 2 of four equal documents begins halfway through"
    );
}

#[tokio::test]
async fn list_highlights_in_position_order_follows_the_book_not_the_clock() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (book_id, uuid) = seed(&pool, "/lib", "Book A").await;
    seed_epub_structure(&pool, book_id).await;

    // Marked back-to-front: creation order and book order disagree, which is
    // the only arrangement that can tell the two apart.
    highlight_at(&pool, user, &uuid, 3).await;
    highlight_at(&pool, user, &uuid, 1).await;
    highlight_at(&pool, user, &uuid, 2).await;

    let by_position = list_highlights(&pool, user, &uuid, AnnotationOrder::Position)
        .await
        .unwrap();
    assert_eq!(
        by_position
            .iter()
            .map(|h| h.spine_index)
            .collect::<Vec<_>>(),
        vec![Some(1), Some(2), Some(3)]
    );

    let by_clock = list_highlights(&pool, user, &uuid, AnnotationOrder::Chronological)
        .await
        .unwrap();
    assert_eq!(
        by_clock
            .iter()
            .map(|h| h.client_id.clone())
            .collect::<Vec<_>>(),
        vec![
            Some("h-3".to_string()),
            Some("h-1".to_string()),
            Some("h-2".to_string())
        ],
        "chronological must stay the order they were marked in"
    );
}

#[tokio::test]
async fn list_highlights_sorts_an_unplaceable_anchor_last_rather_than_dropping_it() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (book_id, uuid) = seed(&pool, "/lib", "Book A").await;
    seed_epub_structure(&pool, book_id).await;

    // The unreadable one is marked *first*, so a list that merely preserved
    // creation order would put it ahead of the placed one and pass by luck.
    create_highlight(
        &pool,
        user,
        &CreateHighlight {
            book_uuid: uuid.clone(),
            epub_cfi_range: "kobo-span-nonsense".into(),
            color: HighlightColor::Amber,
            text: Some("an anchor that cannot be read".into()),
            client_id: Some("h-none".into()),
        },
    )
    .await
    .unwrap();
    highlight_at(&pool, user, &uuid, 2).await;

    let list = list_highlights(&pool, user, &uuid, AnnotationOrder::Position)
        .await
        .unwrap();
    assert_eq!(
        list.len(),
        2,
        "an unplaceable highlight is still the reader's"
    );
    assert_eq!(list[0].spine_index, Some(2));
    assert_eq!(list[1].spine_index, None);
}

#[tokio::test]
async fn list_bookmarks_places_and_orders_the_same_way_highlights_do() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (book_id, uuid) = seed(&pool, "/lib", "Book A").await;
    seed_epub_structure(&pool, book_id).await;

    for spine_index in [3i64, 1] {
        create_bookmark(
            &pool,
            user,
            &CreateBookmark {
                book_uuid: uuid.clone(),
                position: cfi_in_spine(spine_index),
                title: Some(format!("mark {spine_index}")),
                client_id: Some(format!("b-{spine_index}")),
            },
        )
        .await
        .unwrap();
    }

    let list = list_bookmarks(&pool, user, &uuid, AnnotationOrder::Position)
        .await
        .unwrap();
    assert_eq!(
        list.iter().map(|b| b.spine_index).collect::<Vec<_>>(),
        vec![Some(1), Some(3)]
    );
    assert_eq!(list[0].chapter_title.as_deref(), Some("One"));
    assert_eq!(list[1].chapter_title.as_deref(), Some("Three"));
}

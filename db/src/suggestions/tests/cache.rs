//! The cache CRUD: `mark_pending`, `replace_suggestions` persisting rows
//! and the resolved or sticky-empty marker and overwriting prior rows,
//! `delete_suggestions`, and the DB-failure path.

use crate::pool::init_db;
use crate::suggestions::data::{
    delete_suggestions, get_suggestion_cover, get_suggestions, mark_pending, replace_suggestions,
    suggestion_state, NewSuggestion, SuggestionState, SuggestionsDataError,
};

fn new_suggestion(id: i64, title: &str, cover: bool) -> NewSuggestion {
    NewSuggestion {
        hardcover_id: id,
        hardcover_slug: Some(format!("slug-{id}")),
        title: title.to_string(),
        author: "Some Author".to_string(),
        list_count: id,
        cover_mime: cover.then(|| "image/jpeg".to_string()),
        cover_bytes: cover.then(|| b"\xFF\xD8\xFFjpeg".to_vec()),
    }
}

#[tokio::test]
async fn mark_pending_sets_pending_state() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    mark_pending(&pool, "book-1").await.unwrap();
    let (state, _) = suggestion_state(&pool, "book-1").await.unwrap().unwrap();
    assert_eq!(state, SuggestionState::Pending);
}

#[tokio::test]
async fn replace_suggestions_persists_rows_and_resolved_state() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let rows = vec![
        new_suggestion(10, "First", true),
        new_suggestion(20, "Second", false),
    ];
    replace_suggestions(&pool, "book-1", &rows, SuggestionState::Resolved)
        .await
        .unwrap();

    let got = get_suggestions(&pool, "book-1").await.unwrap();
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].rank, 0);
    assert_eq!(got[0].title, "First");
    assert!(got[0].has_cover);
    assert_eq!(got[1].title, "Second");
    assert!(!got[1].has_cover);

    let (state, _) = suggestion_state(&pool, "book-1").await.unwrap().unwrap();
    assert_eq!(state, SuggestionState::Resolved);

    let (mime, bytes) = get_suggestion_cover(&pool, "book-1", 0)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(mime, "image/jpeg");
    assert!(!bytes.is_empty());
    assert!(get_suggestion_cover(&pool, "book-1", 1)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn replace_suggestions_records_sticky_empty_marker() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_suggestions(&pool, "book-1", &[], SuggestionState::Empty)
        .await
        .unwrap();
    assert!(get_suggestions(&pool, "book-1").await.unwrap().is_empty());
    let (state, _) = suggestion_state(&pool, "book-1").await.unwrap().unwrap();
    assert_eq!(state, SuggestionState::Empty);
}

#[tokio::test]
async fn replace_suggestions_overwrites_prior_rows() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_suggestions(
        &pool,
        "book-1",
        &[new_suggestion(1, "Old", false)],
        SuggestionState::Resolved,
    )
    .await
    .unwrap();
    replace_suggestions(
        &pool,
        "book-1",
        &[new_suggestion(2, "New", false)],
        SuggestionState::Resolved,
    )
    .await
    .unwrap();
    let got = get_suggestions(&pool, "book-1").await.unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].title, "New");
}

#[tokio::test]
async fn delete_suggestions_clears_rows_and_marker() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_suggestions(
        &pool,
        "book-1",
        &[new_suggestion(1, "X", false)],
        SuggestionState::Resolved,
    )
    .await
    .unwrap();
    delete_suggestions(&pool, "book-1").await.unwrap();
    assert!(get_suggestions(&pool, "book-1").await.unwrap().is_empty());
    assert!(suggestion_state(&pool, "book-1").await.unwrap().is_none());
}

#[tokio::test]
async fn mark_pending_propagates_db_error_when_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    pool.close().await;
    let err = mark_pending(&pool, "any-uuid").await.unwrap_err();
    assert!(matches!(err, SuggestionsDataError::Db(_)));
}

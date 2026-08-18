//! Tests for the cleanup wrappers' one piece of real logic: how a
//! `db::cleanup` error is mapped onto the wire. The queue/decide behaviour
//! itself is covered in `db/src/cleanup/review/tests.rs`.

use omnibus_db::cleanup::CleanupStoreError;
use omnibus_db::CleanupApplyError;

use super::{map_apply_error, map_store_error};

#[test]
fn map_store_error_genericizes_an_internal_fault() {
    for e in [
        CleanupStoreError::UnknownToken("publisher".into()),
        CleanupStoreError::Payload(serde_json::from_str::<i32>("{").unwrap_err()),
    ] {
        assert!(map_store_error("ctx", e)
            .to_string()
            .contains("internal server error"));
    }
}

#[test]
fn map_store_error_passes_a_review_decision_through_verbatim() {
    for (e, expected) in [
        (CleanupStoreError::NotFound, "suggestion not found"),
        (
            CleanupStoreError::AlreadyReviewed,
            "suggestion has already been reviewed",
        ),
        (
            CleanupStoreError::NotADecision,
            "decision must be accepted or rejected",
        ),
        (
            CleanupStoreError::Unsupported,
            "this suggestion cannot be applied automatically",
        ),
    ] {
        assert!(map_store_error("ctx", e).to_string().contains(expected));
    }
}

#[test]
fn map_store_error_delegates_a_wrapped_apply_failure() {
    let e = CleanupStoreError::Apply(CleanupApplyError::NotFound(7));
    assert!(map_store_error("ctx", e)
        .to_string()
        .contains("entity not found: 7"));
}

#[test]
fn map_apply_error_genericizes_a_db_failure_but_keeps_a_typed_sentence() {
    let db = map_apply_error("ctx", CleanupApplyError::Db(sqlx::Error::RowNotFound));
    assert!(db.to_string().contains("internal server error"));

    let typed = map_apply_error("ctx", CleanupApplyError::NotFound(3));
    assert!(typed.to_string().contains("entity not found: 3"));
}

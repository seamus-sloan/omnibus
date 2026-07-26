//! Unit tests for the `highlights` CRUD module — create round-trip,
//! BookNotFound / NotFound variants, list isolation, color + note
//! updates, and delete behaviour.

use omnibus_shared::EbookMetadata;

use super::*;
use crate::{init_db, replace_books};

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
            word_count: None,
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
        client_id: None,
        book_uuid: uuid.clone(),
        epub_cfi_range: "epubcfi(/6/4!/4/2,/1:0,/1:100)".into(),
        color: HighlightColor::Blue,
        text: Some("the quoted passage".into()),
    };
    let h = create_highlight(&pool, user, &input).await.unwrap();
    assert_eq!(h.book_uuid, uuid);
    assert_eq!(h.epub_cfi_range, "epubcfi(/6/4!/4/2,/1:0,/1:100)");
    assert_eq!(h.color, HighlightColor::Blue);
    assert_eq!(h.text.as_deref(), Some("the quoted passage"));
    assert!(h.note.is_none());
    assert!(h.created_at > 0);
}

#[tokio::test]
async fn create_highlight_returns_book_not_found_for_unknown_uuid() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let input = CreateHighlight {
        client_id: None,
        book_uuid: "no-such-uuid".into(),
        epub_cfi_range: "epubcfi(/6/4)".into(),
        color: HighlightColor::Amber,
        text: None,
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
        client_id: None,
        book_uuid: uuid_a.clone(),
        epub_cfi_range: "epubcfi(/6/4)".into(),
        color: HighlightColor::Amber,
        text: None,
    };
    let input_b = CreateHighlight {
        client_id: None,
        book_uuid: uuid_b.clone(),
        epub_cfi_range: "epubcfi(/6/8)".into(),
        color: HighlightColor::Green,
        text: None,
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
            client_id: None,
            book_uuid: uuid.clone(),
            epub_cfi_range: "epubcfi(/6/4)".into(),
            color: HighlightColor::Amber,
            text: None,
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
            client_id: None,
            book_uuid: uuid.clone(),
            epub_cfi_range: "epubcfi(/6/4)".into(),
            color: HighlightColor::Amber,
            text: None,
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
            client_id: None,
            book_uuid: uuid.clone(),
            epub_cfi_range: "epubcfi(/6/4)".into(),
            color: HighlightColor::Green,
            text: None,
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
            client_id: None,
            book_uuid: uuid.clone(),
            epub_cfi_range: "epubcfi(/6/4)".into(),
            color: HighlightColor::Rose,
            text: None,
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
            client_id: None,
            book_uuid: uuid.clone(),
            epub_cfi_range: "epubcfi(/6/4)".into(),
            color: HighlightColor::Blue,
            text: None,
        },
    )
    .await
    .unwrap();

    let err = delete_highlight(&pool, bob, h.id).await.unwrap_err();
    assert!(matches!(err, HighlightError::NotFound));
}

/// Bulk-insert `count` highlight rows for `(user_id, book_uuid)`
/// bypassing `create_highlight` — the CRUD helper resolves the book
/// uuid on every call, which is fine at 1–2 rows but too slow for a
/// 1500-row response-cap fixture.
async fn seed_highlights_raw(pool: &SqlitePool, user_id: i64, book_uuid: &str, count: i64) {
    sqlx::query(
        r"
        WITH RECURSIVE n(i) AS (
            SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < ?
        )
        INSERT INTO highlights (user_id, book_uuid, epub_cfi_range, color, created_at)
        SELECT ?, ?, 'epubcfi(/' || i || ')', 'amber', i FROM n
        ",
    )
    .bind(count)
    .bind(user_id)
    .bind(book_uuid)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn list_highlights_caps_response_at_hard_limit() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    let over_cap = LIST_HIGHLIGHTS_LIMIT + 500;
    seed_highlights_raw(&pool, user, &uuid, over_cap).await;

    let list = list_highlights(&pool, user, &uuid).await.unwrap();
    assert_eq!(
        list.len() as i64,
        LIST_HIGHLIGHTS_LIMIT,
        "list_highlights must not return more than LIST_HIGHLIGHTS_LIMIT rows",
    );
}

/// Guard against a covering-index regression. Without
/// `idx_highlights_user_book_created` the planner falls back to
/// `idx_highlights_user_book` and sorts the matched rows in memory —
/// SQLite calls this out as `USE TEMP B-TREE FOR ORDER BY` in the plan.
/// We assert the plan mentions the covering index by name and does not
/// mention the temp b-tree — a structural check that survives
/// point-release plan-string wording changes.
#[tokio::test]
async fn list_highlights_query_plan_uses_covering_index() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    // Seed two users so the planner's stats reflect real selectivity —
    // with a single user_id ANALYZE tells the planner the filter buys
    // nothing and it may prefer a plain SCAN.
    let alice = seed_user(&pool, "alice").await;
    let bob = seed_user(&pool, "bob").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    seed_highlights_raw(&pool, alice, &uuid, 500).await;
    seed_highlights_raw(&pool, bob, &uuid, 500).await;
    sqlx::query("ANALYZE").execute(&pool).await.unwrap();

    let plan_rows = sqlx::query(
        "EXPLAIN QUERY PLAN
         SELECT h.id, h.book_uuid, h.epub_cfi_range, h.color, h.note, h.text, h.created_at
           FROM highlights h
          WHERE h.user_id = ? AND h.book_uuid = ?
          ORDER BY h.created_at ASC
          LIMIT ?",
    )
    .bind(alice)
    .bind(&uuid)
    .bind(LIST_HIGHLIGHTS_LIMIT)
    .fetch_all(&pool)
    .await
    .unwrap();
    let plan: String = plan_rows
        .iter()
        .map(|r| r.get::<String, _>("detail"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        plan.contains("idx_highlights_user_book_created"),
        "expected covering index in plan, got:\n{plan}",
    );
    assert!(
        !plan.contains("USE TEMP B-TREE FOR ORDER BY"),
        "expected index-only sort — plan still uses a temp b-tree:\n{plan}",
    );
}

#[tokio::test]
async fn create_highlight_propagates_db_error_when_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    pool.close().await;
    let err = create_highlight(
        &pool,
        1,
        &CreateHighlight {
            client_id: None,
            book_uuid: "any-uuid".into(),
            epub_cfi_range: "epubcfi(/6/2!/4/2)".into(),
            color: HighlightColor::Amber,
            text: None,
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(err, HighlightError::Sqlx(_)));
}

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

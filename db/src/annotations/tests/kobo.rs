//! The Kobo channel: `ingest_kobo_annotations` upserting on the
//! device-minted id (replay-safe, CFI kept or dropped by anchor movement,
//! deletes and batches), and `served_kobo_annotations` in its single-uuid
//! and batch forms with anchor filtering, per-book caps, grouping, and the
//! DB-failure paths.

use super::super::*;
use super::{seed, seed_user};
use crate::init_db;

// Kobo ingest / serve (#1278)
fn kobo_upload(client_id: &str, color: HighlightColor, note: Option<&str>) -> IngestKoboAnnotation {
    IngestKoboAnnotation {
        client_id: client_id.into(),
        color,
        text: Some("device prose".into()),
        note: note.map(Into::into),
        kobo_location: r#"{"span":{"startPath":"span#kobo\\.1\\.2","startChar":3}}"#.into(),
        epub_cfi_range: None,
    }
}

#[tokio::test]
async fn ingest_kobo_annotations_creates_anchorless_rows_the_web_list_still_returns() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;

    ingest_kobo_annotations(
        &pool,
        user,
        &uuid,
        &[kobo_upload(
            "kobo-1",
            HighlightColor::Green,
            Some("device note"),
        )],
        &[],
    )
    .await
    .unwrap();

    let listed = list_highlights(&pool, user, &uuid).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(
        listed[0].epub_cfi_range, None,
        "no CFI unless the server derived one"
    );
    assert_eq!(listed[0].color, HighlightColor::Green);
    assert_eq!(listed[0].note.as_deref(), Some("device note"));
    assert_eq!(listed[0].text.as_deref(), Some("device prose"));
    assert_eq!(listed[0].client_id.as_deref(), Some("kobo-1"));

    let served = served_kobo_annotations(&pool, user, &uuid).await.unwrap();
    assert_eq!(served.len(), 1);
    assert_eq!(served[0].client_id, "kobo-1");
    assert!(served[0].kobo_location.contains("startPath"));
}

#[tokio::test]
async fn ingest_kobo_annotations_replay_of_the_same_upload_creates_no_duplicates() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    let batch = [kobo_upload("kobo-1", HighlightColor::Amber, None)];

    ingest_kobo_annotations(&pool, user, &uuid, &batch, &[])
        .await
        .unwrap();
    ingest_kobo_annotations(&pool, user, &uuid, &batch, &[])
        .await
        .unwrap();

    assert_eq!(list_highlights(&pool, user, &uuid).await.unwrap().len(), 1);
}

#[tokio::test]
async fn ingest_kobo_annotations_updates_color_note_and_text_for_an_existing_id() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;

    ingest_kobo_annotations(
        &pool,
        user,
        &uuid,
        &[kobo_upload("kobo-1", HighlightColor::Amber, None)],
        &[],
    )
    .await
    .unwrap();

    // A device-side edit re-uploads the same id with newer content; unlike
    // the web path's replayed create, this carries intent and must win.
    let mut edited = kobo_upload("kobo-1", HighlightColor::Violet, Some("second thoughts"));
    edited.text = Some("re-selected prose".into());
    ingest_kobo_annotations(&pool, user, &uuid, &[edited], &[])
        .await
        .unwrap();

    let listed = list_highlights(&pool, user, &uuid).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].color, HighlightColor::Violet);
    assert_eq!(listed[0].note.as_deref(), Some("second thoughts"));
    assert_eq!(listed[0].text.as_deref(), Some("re-selected prose"));
}

#[tokio::test]
async fn ingest_kobo_annotations_stores_a_derived_cfi_alongside_the_kobo_anchor() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    let mut upload = kobo_upload("kobo-1", HighlightColor::Amber, None);
    upload.epub_cfi_range = Some("epubcfi(/6/2!/4/4,/1:0,/1:20)".into());

    ingest_kobo_annotations(&pool, user, &uuid, &[upload], &[])
        .await
        .unwrap();

    let listed = list_highlights(&pool, user, &uuid).await.unwrap();
    assert_eq!(
        listed[0].epub_cfi_range.as_deref(),
        Some("epubcfi(/6/2!/4/4,/1:0,/1:20)")
    );
    // Still Kobo-placeable: the kobo_location anchor rides along.
    let served = served_kobo_annotations(&pool, user, &uuid).await.unwrap();
    assert_eq!(served.len(), 1);
}

#[tokio::test]
async fn ingest_kobo_annotations_keeps_an_existing_cfi_when_the_anchor_is_unchanged() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    let mut first = kobo_upload("kobo-1", HighlightColor::Amber, None);
    first.epub_cfi_range = Some("epubcfi(/6/2!/4/4,/1:0,/1:20)".into());
    ingest_kobo_annotations(&pool, user, &uuid, &[first], &[])
        .await
        .unwrap();

    // A color edit re-uploads the same anchor; derivation may fail (no
    // kepub cache right now) but the stored CFI is still truthful.
    let replay = kobo_upload("kobo-1", HighlightColor::Blue, None);
    ingest_kobo_annotations(&pool, user, &uuid, &[replay], &[])
        .await
        .unwrap();

    let listed = list_highlights(&pool, user, &uuid).await.unwrap();
    assert_eq!(listed[0].color, HighlightColor::Blue);
    assert_eq!(
        listed[0].epub_cfi_range.as_deref(),
        Some("epubcfi(/6/2!/4/4,/1:0,/1:20)")
    );
}

#[tokio::test]
async fn ingest_kobo_annotations_drops_a_stale_cfi_when_the_anchor_moves_underivably() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    let mut first = kobo_upload("kobo-1", HighlightColor::Amber, None);
    first.epub_cfi_range = Some("epubcfi(/6/2!/4/4,/1:0,/1:20)".into());
    ingest_kobo_annotations(&pool, user, &uuid, &[first], &[])
        .await
        .unwrap();

    // The device moved the highlight and this time nothing could be
    // derived: keeping the old CFI would render the wrong passage.
    let mut moved = kobo_upload("kobo-1", HighlightColor::Amber, None);
    moved.kobo_location = r#"{"span":{"startPath":"span#kobo\\.9\\.9","startChar":0}}"#.into();
    ingest_kobo_annotations(&pool, user, &uuid, &[moved], &[])
        .await
        .unwrap();

    let listed = list_highlights(&pool, user, &uuid).await.unwrap();
    assert_eq!(listed[0].epub_cfi_range, None);
}

#[tokio::test]
async fn ingest_kobo_annotations_deletes_rows_by_device_minted_id() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;

    ingest_kobo_annotations(
        &pool,
        user,
        &uuid,
        &[
            kobo_upload("kobo-1", HighlightColor::Amber, None),
            kobo_upload("kobo-2", HighlightColor::Blue, None),
        ],
        &[],
    )
    .await
    .unwrap();

    ingest_kobo_annotations(&pool, user, &uuid, &[], &["kobo-1".to_string()])
        .await
        .unwrap();

    let served = served_kobo_annotations(&pool, user, &uuid).await.unwrap();
    assert_eq!(served.len(), 1);
    assert_eq!(served[0].client_id, "kobo-2");
}

#[tokio::test]
async fn ingest_kobo_annotations_applies_a_multi_row_batch_conflict_insert_and_delete_together() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;

    // Pre-existing rows: kobo-1 will be updated in place, kobo-3 will be
    // deleted, both inside the same batched call under test.
    ingest_kobo_annotations(
        &pool,
        user,
        &uuid,
        &[
            kobo_upload("kobo-1", HighlightColor::Amber, None),
            kobo_upload("kobo-3", HighlightColor::Blue, None),
        ],
        &[],
    )
    .await
    .unwrap();

    // One call carries a conflicting update (kobo-1), a fresh insert
    // (kobo-2), and a delete (kobo-3) — exercising the chunked multi-row
    // VALUES upsert and the IN (...) delete together.
    let mut edited_kobo_1 = kobo_upload("kobo-1", HighlightColor::Violet, Some("edited"));
    edited_kobo_1.text = Some("re-selected prose".into());
    ingest_kobo_annotations(
        &pool,
        user,
        &uuid,
        &[
            edited_kobo_1,
            kobo_upload("kobo-2", HighlightColor::Green, None),
        ],
        &["kobo-3".to_string()],
    )
    .await
    .unwrap();

    let mut listed = list_highlights(&pool, user, &uuid).await.unwrap();
    listed.sort_by(|a, b| a.client_id.cmp(&b.client_id));
    assert_eq!(listed.len(), 2, "kobo-3 deleted, kobo-1 and kobo-2 remain");
    assert_eq!(listed[0].client_id.as_deref(), Some("kobo-1"));
    assert_eq!(listed[0].color, HighlightColor::Violet);
    assert_eq!(listed[0].note.as_deref(), Some("edited"));
    assert_eq!(listed[0].text.as_deref(), Some("re-selected prose"));
    assert_eq!(listed[1].client_id.as_deref(), Some("kobo-2"));
    assert_eq!(listed[1].color, HighlightColor::Green);

    let served = served_kobo_annotations(&pool, user, &uuid).await.unwrap();
    assert_eq!(served.len(), 2);
    assert!(served.iter().all(|s| s.client_id != "kobo-3"));
}

#[tokio::test]
async fn ingest_kobo_annotations_returns_book_not_found_for_an_unknown_uuid() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;

    let err = ingest_kobo_annotations(
        &pool,
        user,
        "no-such-uuid",
        &[kobo_upload("kobo-1", HighlightColor::Amber, None)],
        &[],
    )
    .await
    .unwrap_err();
    assert!(matches!(err, HighlightError::BookNotFound));
}

#[tokio::test]
async fn served_kobo_annotations_excludes_cfi_only_rows_and_other_users() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let alice = seed_user(&pool, "alice").await;
    let bob = seed_user(&pool, "bob").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;

    // Alice: one web highlight (CFI, no kobo anchor) and one device upload.
    create_highlight(
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
    ingest_kobo_annotations(
        &pool,
        alice,
        &uuid,
        &[kobo_upload("kobo-alice", HighlightColor::Amber, None)],
        &[],
    )
    .await
    .unwrap();
    // Bob's device upload on the same book stays his.
    ingest_kobo_annotations(
        &pool,
        bob,
        &uuid,
        &[kobo_upload("kobo-bob", HighlightColor::Rose, None)],
        &[],
    )
    .await
    .unwrap();

    let served = served_kobo_annotations(&pool, alice, &uuid).await.unwrap();
    assert_eq!(served.len(), 1, "web CFI rows and Bob's rows are excluded");
    assert_eq!(served[0].client_id, "kobo-alice");
}

#[tokio::test]
async fn served_kobo_annotations_returns_empty_for_an_unknown_book() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    assert!(served_kobo_annotations(&pool, user, "no-such-uuid")
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn served_kobo_annotations_batch_caps_each_book_at_list_highlights_limit_like_the_single_uuid_form(
) {
    // Regression: an earlier version of the batched query had no per-book
    // cap, so a book past LIST_HIGHLIGHTS_LIMIT rows produced a fingerprint
    // that could never match the one `served_kobo_annotations` acked via the
    // GET path — `checkforchanges` would report that book changed forever.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Big Book").await;

    let total = LIST_HIGHLIGHTS_LIMIT + 5;
    for i in 0..total {
        sqlx::query(
            "INSERT INTO annotations
                 (user_id, book_uuid, kobo_location, color, client_id, created_at)
             VALUES (?, ?, ?, 'amber', ?, ?)",
        )
        .bind(user)
        .bind(&uuid)
        .bind(format!(r#"{{"span":{{"i":{i}}}}}"#))
        .bind(format!("kobo-{i:05}"))
        .bind(i) // explicit, strictly increasing created_at — avoids same-second ties
        .execute(&pool)
        .await
        .unwrap();
    }

    let single = served_kobo_annotations(&pool, user, &uuid).await.unwrap();
    assert_eq!(single.len(), LIST_HIGHLIGHTS_LIMIT as usize);

    let batch = served_kobo_annotations_batch(&pool, user, std::slice::from_ref(&uuid))
        .await
        .unwrap();
    let batched = batch.get(&uuid).cloned().unwrap_or_default();
    assert_eq!(batched.len(), LIST_HIGHLIGHTS_LIMIT as usize);

    assert_eq!(
        crate::kobo::annotations::fingerprint(&single),
        crate::kobo::annotations::fingerprint(&batched),
        "the batched fetch must cap to the same LIST_HIGHLIGHTS_LIMIT window as the \
         single-uuid form, or a device's GET-acked fingerprint can never match again"
    );
}

#[tokio::test]
async fn served_kobo_annotations_batch_groups_rows_by_book_and_omits_empty_and_unknown_uuids() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let alice = seed_user(&pool, "alice").await;
    let (_, book_a) = seed(&pool, "/lib", "Book A").await;
    let (_, book_b) = seed(&pool, "/lib", "Book B").await;
    let (_, book_c) = seed(&pool, "/lib", "Book C").await;

    ingest_kobo_annotations(
        &pool,
        alice,
        &book_a,
        &[
            kobo_upload("kobo-a1", HighlightColor::Amber, None),
            kobo_upload("kobo-a2", HighlightColor::Blue, None),
        ],
        &[],
    )
    .await
    .unwrap();
    ingest_kobo_annotations(
        &pool,
        alice,
        &book_b,
        &[kobo_upload("kobo-b1", HighlightColor::Rose, None)],
        &[],
    )
    .await
    .unwrap();
    // Book C has no Kobo-anchored annotations at all.

    let batch = served_kobo_annotations_batch(
        &pool,
        alice,
        &[
            book_a.clone(),
            book_b.clone(),
            book_c,
            "no-such-uuid".into(),
        ],
    )
    .await
    .unwrap();

    assert_eq!(batch.len(), 2, "only books with servable rows are present");
    let a = batch.get(&book_a).unwrap();
    assert_eq!(a.len(), 2);
    let b = batch.get(&book_b).unwrap();
    assert_eq!(b.len(), 1);
    assert_eq!(b[0].client_id, "kobo-b1");

    // Matches the per-uuid form for the same candidates.
    let a_single = served_kobo_annotations(&pool, alice, &book_a)
        .await
        .unwrap();
    assert_eq!(a.len(), a_single.len());
}

#[tokio::test]
async fn served_kobo_annotations_batch_returns_empty_map_for_no_candidates() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    assert!(served_kobo_annotations_batch(&pool, user, &[])
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn served_kobo_annotations_batch_propagates_db_error_when_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    pool.close().await;
    let err = served_kobo_annotations_batch(&pool, 1, &["uuid".to_string()])
        .await
        .unwrap_err();
    assert!(matches!(err, HighlightError::Sqlx(_)));
}

#[tokio::test]
async fn ingest_kobo_annotations_propagates_db_error_when_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    pool.close().await;
    let err = ingest_kobo_annotations(&pool, 1, "uuid", &[], &[])
        .await
        .unwrap_err();
    assert!(matches!(err, HighlightError::Sqlx(_)));
}

#[tokio::test]
async fn served_kobo_annotations_propagates_db_error_when_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    pool.close().await;
    let err = served_kobo_annotations(&pool, 1, "uuid").await.unwrap_err();
    assert!(matches!(err, HighlightError::Sqlx(_)));
}

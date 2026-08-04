//! Unit tests for the per-(device, book) annotation sync state: adoption
//! lifecycle, ack watermarks, the changed-books report (incl. the AC5
//! unadopted-empty guard), and fingerprint behavior.

use super::*;
use crate::annotations::{ingest_kobo_annotations, served_kobo_annotations, IngestKoboAnnotation};
use crate::init_db;
use crate::test_support::seed_synced_ebook;
use omnibus_shared::HighlightColor;

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

/// One (pool, user, device, book) fixture with nothing ingested yet.
async fn fixture() -> (SqlitePool, i64, i64, String) {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "kobo-reader").await;
    let device = crate::kobo_devices::create_device(&pool, user, "Kobo")
        .await
        .unwrap();
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    (pool, user, device.id, uuid)
}

fn upload(client_id: &str, color: HighlightColor, note: Option<&str>) -> IngestKoboAnnotation {
    IngestKoboAnnotation {
        client_id: client_id.into(),
        color,
        text: Some("highlighted prose".into()),
        note: note.map(Into::into),
        kobo_location: r#"{"span":{"startPath":"span#kobo\\.1\\.1","startChar":0}}"#.into(),
        epub_cfi_range: None,
    }
}

#[tokio::test]
async fn is_adopted_is_false_until_mark_adopted_records_the_pair() {
    let (pool, _user, device, uuid) = fixture().await;

    assert!(!is_adopted(&pool, device, &uuid).await.unwrap());
    mark_adopted(&pool, device, &uuid).await.unwrap();
    assert!(is_adopted(&pool, device, &uuid).await.unwrap());
}

#[tokio::test]
async fn mark_adopted_is_idempotent_and_preserves_the_first_timestamp() {
    let (pool, _user, device, uuid) = fixture().await;

    mark_adopted(&pool, device, &uuid).await.unwrap();
    sqlx::query("UPDATE kobo_annotations_sync SET adopted_at = 42 WHERE device_id = ?")
        .bind(device)
        .execute(&pool)
        .await
        .unwrap();
    mark_adopted(&pool, device, &uuid).await.unwrap();

    let adopted_at: i64 = sqlx::query_scalar(
        "SELECT adopted_at FROM kobo_annotations_sync WHERE device_id = ? AND book_uuid = ?",
    )
    .bind(device)
    .bind(&uuid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(adopted_at, 42);
}

#[tokio::test]
async fn changed_book_uuids_reports_a_book_whose_fingerprint_moved_past_the_ack() {
    let (pool, user, device, uuid) = fixture().await;

    ingest_kobo_annotations(
        &pool,
        user,
        &uuid,
        &[upload("kobo-a", HighlightColor::Amber, None)],
        &[],
    )
    .await
    .unwrap();
    mark_adopted(&pool, device, &uuid).await.unwrap();

    // Never acked -> reported.
    assert_eq!(
        changed_book_uuids(&pool, user, device).await.unwrap(),
        vec![uuid.clone()]
    );

    // Ack the current set -> quiet, but only once the device is on record as
    // holding the book (#1647) — the ack gate mark_downloaded satisfies.
    mark_downloaded(&pool, device, &uuid).await.unwrap();
    let served = served_kobo_annotations(&pool, user, &uuid).await.unwrap();
    ack_served(&pool, device, &uuid, &fingerprint(&served))
        .await
        .unwrap();
    assert!(changed_book_uuids(&pool, user, device)
        .await
        .unwrap()
        .is_empty());

    // A web-side recolor moves the fingerprint -> reported again.
    let id = crate::annotations::highlight_id_for_client_id(&pool, user, "kobo-a")
        .await
        .unwrap()
        .expect("ingested row resolves by client_id");
    crate::annotations::update_highlight_color(&pool, user, id, HighlightColor::Violet)
        .await
        .unwrap();
    assert_eq!(
        changed_book_uuids(&pool, user, device).await.unwrap(),
        vec![uuid]
    );
}

#[tokio::test]
async fn changed_book_uuids_never_reports_an_unadopted_empty_pair() {
    let (pool, user, device, uuid) = fixture().await;

    // The pair exists in sync state (an unadopted GET touched nothing; seed a
    // bare ack row to prove even that shape stays quiet) but has no
    // annotations and no adoption — reporting it would invite the
    // backlog-wiping empty GET (AC5).
    sqlx::query("INSERT INTO kobo_annotations_sync (device_id, book_uuid) VALUES (?, ?)")
        .bind(device)
        .bind(&uuid)
        .execute(&pool)
        .await
        .unwrap();

    assert!(changed_book_uuids(&pool, user, device)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn changed_book_uuids_reports_an_unadopted_pair_once_another_device_populated_it() {
    let (pool, user, device_a, uuid) = fixture().await;
    let device_b = crate::kobo_devices::create_device(&pool, user, "Second Kobo")
        .await
        .unwrap()
        .id;

    // Device A uploads its annotations; device B has never PATCHed this book.
    ingest_kobo_annotations(
        &pool,
        user,
        &uuid,
        &[upload("kobo-a", HighlightColor::Green, None)],
        &[],
    )
    .await
    .unwrap();
    mark_adopted(&pool, device_a, &uuid).await.unwrap();

    // The servable set is non-empty, so device B is told to fetch it — same
    // rule as the GET handler's `!served.is_empty()` arm.
    assert_eq!(
        changed_book_uuids(&pool, user, device_b).await.unwrap(),
        vec![uuid]
    );
}

#[tokio::test]
async fn changed_book_uuids_reports_a_book_after_a_web_side_delete() {
    let (pool, user, device, uuid) = fixture().await;

    ingest_kobo_annotations(
        &pool,
        user,
        &uuid,
        &[upload("kobo-a", HighlightColor::Amber, None)],
        &[],
    )
    .await
    .unwrap();
    mark_adopted(&pool, device, &uuid).await.unwrap();
    mark_downloaded(&pool, device, &uuid).await.unwrap();
    let served = served_kobo_annotations(&pool, user, &uuid).await.unwrap();
    ack_served(&pool, device, &uuid, &fingerprint(&served))
        .await
        .unwrap();
    assert!(changed_book_uuids(&pool, user, device)
        .await
        .unwrap()
        .is_empty());

    // Deleting the highlight in the web UI must re-surface the book so the
    // device fetches the now-smaller set and reconciles by omission (AC4).
    let id = crate::annotations::highlight_id_for_client_id(&pool, user, "kobo-a")
        .await
        .unwrap()
        .unwrap();
    crate::annotations::delete_highlight(&pool, user, id)
        .await
        .unwrap();
    assert_eq!(
        changed_book_uuids(&pool, user, device).await.unwrap(),
        vec![uuid]
    );
}

#[tokio::test]
async fn changed_book_uuids_computes_the_correct_set_across_multiple_candidate_books() {
    // Exercises the batched fetch (`served_kobo_annotations_batch`) with more
    // than one candidate in flight: an acked-and-quiet book, a never-acked
    // book, and a book whose ack went stale after a web-side recolor.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "kobo-reader").await;
    let device = crate::kobo_devices::create_device(&pool, user, "Kobo")
        .await
        .unwrap()
        .id;
    let quiet = seed_synced_ebook(&pool, "quiet.epub", "Quiet", "A").await;
    let never_acked = seed_synced_ebook(&pool, "never.epub", "Never Acked", "B").await;
    let stale = seed_synced_ebook(&pool, "stale.epub", "Stale", "C").await;

    for (uuid, client_id) in [
        (&quiet, "kobo-quiet"),
        (&never_acked, "kobo-never"),
        (&stale, "kobo-stale"),
    ] {
        ingest_kobo_annotations(
            &pool,
            user,
            uuid,
            &[upload(client_id, HighlightColor::Amber, None)],
            &[],
        )
        .await
        .unwrap();
        mark_adopted(&pool, device, uuid).await.unwrap();
        mark_downloaded(&pool, device, uuid).await.unwrap();
    }

    let quiet_served = served_kobo_annotations(&pool, user, &quiet).await.unwrap();
    ack_served(&pool, device, &quiet, &fingerprint(&quiet_served))
        .await
        .unwrap();

    let stale_served = served_kobo_annotations(&pool, user, &stale).await.unwrap();
    ack_served(&pool, device, &stale, &fingerprint(&stale_served))
        .await
        .unwrap();
    let stale_id = crate::annotations::highlight_id_for_client_id(&pool, user, "kobo-stale")
        .await
        .unwrap()
        .expect("ingested row resolves by client_id");
    crate::annotations::update_highlight_color(&pool, user, stale_id, HighlightColor::Violet)
        .await
        .unwrap();

    let mut changed = changed_book_uuids(&pool, user, device).await.unwrap();
    changed.sort();
    let mut expected = vec![never_acked, stale];
    expected.sort();
    assert_eq!(
        changed, expected,
        "quiet stays out; never-acked and stale-ack both report"
    );
}

#[tokio::test]
async fn changed_book_uuids_is_scoped_to_the_devices_user() {
    let (pool, alice, _alice_device, uuid) = fixture().await;
    let bob = seed_user(&pool, "bob").await;
    let bobs_device = crate::kobo_devices::create_device(&pool, bob, "Bob Kobo")
        .await
        .unwrap()
        .id;

    ingest_kobo_annotations(
        &pool,
        alice,
        &uuid,
        &[upload("kobo-a", HighlightColor::Amber, None)],
        &[],
    )
    .await
    .unwrap();

    // Alice's annotations never surface through Bob's device.
    assert!(changed_book_uuids(&pool, bob, bobs_device)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn fingerprint_is_order_independent_and_reacts_to_content_and_membership() {
    let a = ServedKoboAnnotation {
        client_id: "a".into(),
        color: HighlightColor::Amber,
        text: Some("t".into()),
        note: None,
        kobo_location: "{}".into(),
        updated_at: 1,
    };
    let b = ServedKoboAnnotation {
        client_id: "b".into(),
        color: HighlightColor::Blue,
        text: None,
        note: Some("n".into()),
        kobo_location: "{}".into(),
        updated_at: 2,
    };

    let fp_ab = fingerprint(&[a.clone(), b.clone()]);
    let fp_ba = fingerprint(&[b.clone(), a.clone()]);
    assert_eq!(fp_ab, fp_ba, "serve order must not move the fingerprint");

    let mut recolored = a.clone();
    recolored.color = HighlightColor::Rose;
    assert_ne!(fingerprint(&[recolored, b.clone()]), fp_ab);

    let mut noted = a.clone();
    noted.note = Some("added".into());
    assert_ne!(fingerprint(&[noted, b.clone()]), fp_ab);

    assert_ne!(
        fingerprint(std::slice::from_ref(&a)),
        fp_ab,
        "membership must move it"
    );
    assert_ne!(fingerprint(&[]), fp_ab);

    // Anchors and timestamps are deliberately excluded.
    let mut moved = a.clone();
    moved.kobo_location = r#"{"other":1}"#.into();
    moved.updated_at = 99;
    assert_eq!(fingerprint(&[moved, b]), fp_ab);
}

#[tokio::test]
async fn fingerprint_distinguishes_a_none_field_from_an_empty_string() {
    let base = ServedKoboAnnotation {
        client_id: "a".into(),
        color: HighlightColor::Amber,
        text: None,
        note: None,
        kobo_location: "{}".into(),
        updated_at: 1,
    };
    let mut empty = base.clone();
    empty.note = Some(String::new());
    assert_ne!(fingerprint(&[base]), fingerprint(&[empty]));
}

#[tokio::test]
async fn is_adopted_propagates_db_error_when_pool_is_closed() {
    let (pool, _user, device, uuid) = fixture().await;
    pool.close().await;
    assert!(matches!(
        is_adopted(&pool, device, &uuid).await,
        Err(KoboAnnotationSyncError::Sqlx(_))
    ));
}

#[tokio::test]
async fn mark_adopted_propagates_db_error_when_pool_is_closed() {
    let (pool, _user, device, uuid) = fixture().await;
    pool.close().await;
    assert!(matches!(
        mark_adopted(&pool, device, &uuid).await,
        Err(KoboAnnotationSyncError::Sqlx(_))
    ));
}

#[tokio::test]
async fn ack_served_propagates_db_error_when_pool_is_closed() {
    let (pool, _user, device, uuid) = fixture().await;
    pool.close().await;
    assert!(matches!(
        ack_served(&pool, device, &uuid, "fp").await,
        Err(KoboAnnotationSyncError::Sqlx(_))
    ));
}

/// #1647: a GET drained for a book this device never fetched over `download`
/// must not stick — `ack_served` becomes a no-op, so the pair stays
/// reportable until the device actually holds the file.
#[tokio::test]
async fn ack_served_is_a_no_op_for_a_book_the_device_has_not_downloaded() {
    let (pool, user, device, uuid) = fixture().await;

    ingest_kobo_annotations(
        &pool,
        user,
        &uuid,
        &[upload("kobo-a", HighlightColor::Amber, None)],
        &[],
    )
    .await
    .unwrap();
    mark_adopted(&pool, device, &uuid).await.unwrap();
    let served = served_kobo_annotations(&pool, user, &uuid).await.unwrap();
    ack_served(&pool, device, &uuid, &fingerprint(&served))
        .await
        .unwrap();

    assert_eq!(
        changed_book_uuids(&pool, user, device).await.unwrap(),
        vec![uuid.clone()],
        "the ack never took hold, so the book stays reportable"
    );
    let acked: Option<String> = sqlx::query_scalar(
        "SELECT acked_fingerprint FROM kobo_annotations_sync WHERE device_id = ? AND book_uuid = ?",
    )
    .bind(device)
    .bind(&uuid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(acked.is_none());
}

#[tokio::test]
async fn mark_downloaded_records_the_pair_and_is_idempotent() {
    let (pool, _user, device, uuid) = fixture().await;

    let downloaded_at: Option<i64> = sqlx::query_scalar(
        "SELECT downloaded_at FROM kobo_annotations_sync WHERE device_id = ? AND book_uuid = ?",
    )
    .bind(device)
    .bind(&uuid)
    .fetch_optional(&pool)
    .await
    .unwrap();
    assert!(downloaded_at.is_none(), "no row until the first download");

    mark_downloaded(&pool, device, &uuid).await.unwrap();
    mark_downloaded(&pool, device, &uuid).await.unwrap();

    let downloaded_at: Option<i64> = sqlx::query_scalar(
        "SELECT downloaded_at FROM kobo_annotations_sync WHERE device_id = ? AND book_uuid = ?",
    )
    .bind(device)
    .bind(&uuid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(downloaded_at.is_some());
}

#[tokio::test]
async fn mark_downloaded_propagates_db_error_when_pool_is_closed() {
    let (pool, _user, device, uuid) = fixture().await;
    pool.close().await;
    assert!(matches!(
        mark_downloaded(&pool, device, &uuid).await,
        Err(KoboAnnotationSyncError::Sqlx(_))
    ));
}

#[tokio::test]
async fn changed_book_uuids_propagates_db_error_when_pool_is_closed() {
    let (pool, user, device, _uuid) = fixture().await;
    pool.close().await;
    assert!(matches!(
        changed_book_uuids(&pool, user, device).await,
        Err(KoboAnnotationSyncError::Sqlx(_))
    ));
}

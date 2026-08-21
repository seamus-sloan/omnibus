//! `/v1/library/sync`: the entitlement delta a device pages through — shelf
//! opt-in scoping, the continue loop, format and size advertisement, and the
//! reading state each entitlement carries.

use axum::http::StatusCode;
use omnibus_db::{self as db, test_support::seed_synced_ebook};
use serde_json::Value;
use tower::ServiceExt;

use crate::auth::test_support as auth_test_support;

use super::{body_json, fixture, get, opt_in, pin_state_clocks, seed_book_with_kepub_cache};

#[tokio::test]
async fn library_sync_rejects_an_invalid_token() {
    let (app, _pool, _token, _uid) = fixture().await;
    let res = app
        .oneshot(get("/kobo/not-a-real-token/v1/library/sync".to_owned()))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn library_sync_delivers_every_book_across_the_continue_loop() {
    // 150 books > SYNC_PAGE_SIZE (100): exercises the real continue loop, unlike Calibre-Web's SYNC_ITEM_LIMIT nothing is dropped.
    let (app, pool, token, uid) = fixture().await;
    let mut uuids = Vec::new();
    for i in 0..150 {
        uuids.push(
            seed_synced_ebook(
                &pool,
                &format!("b{i}.epub"),
                &format!("Title {i}"),
                "Author",
            )
            .await,
        );
    }
    opt_in(&pool, uid, &uuids).await;

    let mut total = 0;
    let mut pages = 0;
    loop {
        let res = app
            .clone()
            .oneshot(get(format!("/kobo/{token}/v1/library/sync")))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers().get("x-kobo-synctoken").unwrap(), "omnibus");
        let more = res.headers().get("x-kobo-sync").is_some();
        if more {
            assert_eq!(res.headers().get("x-kobo-sync").unwrap(), "continue");
        }
        total += body_json(res).await.as_array().unwrap().len();
        pages += 1;
        assert!(pages <= 3, "continue loop failed to terminate");
        if !more {
            break;
        }
    }

    assert_eq!(total, 150);
    assert_eq!(pages, 2, "150 books should page as 100 + 50");
}

#[tokio::test]
async fn library_sync_omits_the_continue_header_when_one_page_suffices() {
    let (app, pool, token, uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    opt_in(&pool, uid, std::slice::from_ref(&uuid)).await;

    let res = app
        .oneshot(get(format!("/kobo/{token}/v1/library/sync")))
        .await
        .unwrap();

    assert!(res.headers().get("x-kobo-sync").is_none());
    assert_eq!(body_json(res).await.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn library_sync_emits_new_entitlement_pointing_at_download() {
    let (app, pool, token, uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Frank Herbert").await;
    opt_in(&pool, uid, std::slice::from_ref(&uuid)).await;
    let res = app
        .oneshot(get(format!("/kobo/{token}/v1/library/sync")))
        .await
        .unwrap();

    let json = body_json(res).await;
    let ent = &json.as_array().unwrap()[0]["NewEntitlement"];
    assert_eq!(ent["BookEntitlement"]["Id"], uuid);
    assert_eq!(ent["BookMetadata"]["Title"], "Dune");
    assert_eq!(
        ent["BookMetadata"]["ContributorRoles"][0]["Name"],
        "Frank Herbert"
    );
    let url = ent["BookMetadata"]["DownloadUrls"][0]["Url"]
        .as_str()
        .unwrap();
    assert!(
        url.contains(&token),
        "download url should carry the path token"
    );
    assert!(
        url.contains(&uuid),
        "download url should carry the book uuid"
    );
    assert_eq!(
        ent["BookMetadata"]["DownloadUrls"][0]["Format"], "KEPUB",
        "an EPUB-bearing book keeps advertising the KEPUB conversion"
    );
}

#[tokio::test]
async fn library_sync_advertises_the_cbz_format_and_size_for_a_cbz_only_book() {
    let (app, pool, token, uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "berserk-v04.cbz", "Berserk v04", "Kentaro Miura").await;
    sqlx::query(
        "UPDATE book_files SET size_bytes = 777 \
         WHERE book_id = (SELECT id FROM books WHERE uuid = ?)",
    )
    .bind(&uuid)
    .execute(&pool)
    .await
    .unwrap();
    opt_in(&pool, uid, std::slice::from_ref(&uuid)).await;

    let res = app
        .oneshot(get(format!("/kobo/{token}/v1/library/sync")))
        .await
        .unwrap();

    let json = body_json(res).await;
    let dl = &json.as_array().unwrap()[0]["NewEntitlement"]["BookMetadata"]["DownloadUrls"][0];
    assert_eq!(
        dl["Format"], "CBZ",
        "a CBZ-only book must not advertise a KEPUB it can never serve"
    );
    assert_eq!(dl["Size"], 777, "size falls back to the CBZ file's bytes");
}

#[tokio::test]
async fn library_sync_returns_nothing_when_no_shelf_is_opted_in() {
    // AC1: an indexed library with no `sync_to_kobo` shelf syncs nothing. The
    // gate is the default, not an opt-out.
    let (app, pool, token, _uid) = fixture().await;
    seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;

    let res = app
        .oneshot(get(format!("/kobo/{token}/v1/library/sync")))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::OK);
    assert!(body_json(res).await.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn library_sync_excludes_a_book_on_an_unflagged_shelf() {
    // AC1: shelf membership alone is not enough — the shelf must be flagged.
    let (app, pool, token, uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    db::shelves::create_shelf(
        &pool,
        uid,
        &omnibus_shared::CreateShelfRequest {
            kind: omnibus_shared::ShelfKind::Manual,
            name: "Not synced".into(),
            description: None,
            visibility: Default::default(),
            match_mode: None,
            rules: Vec::new(),
            book_uuids: vec![uuid],
        },
    )
    .await
    .unwrap();

    let res = app
        .oneshot(get(format!("/kobo/{token}/v1/library/sync")))
        .await
        .unwrap();

    assert!(body_json(res).await.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn library_sync_never_returns_another_users_opted_in_books() {
    // AC4: the opt-in is scoped through shelf ownership, so one user's flagged
    // shelf is invisible to another user's device token.
    let (app, pool, token, _uid) = fixture().await;
    let other = auth_test_support::create_user(&pool, "other-reader").await;
    let theirs = seed_synced_ebook(&pool, "theirs.epub", "Theirs", "B").await;
    opt_in(&pool, other.id, &[theirs]).await;

    let res = app
        .oneshot(get(format!("/kobo/{token}/v1/library/sync")))
        .await
        .unwrap();

    assert!(body_json(res).await.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn library_sync_reflects_an_opt_in_toggled_off() {
    // AC2: toggling the flag changes what the *next* sync returns, with no
    // intermediate publish step. Since the per-device delta (#922) the device
    // is told to *archive* the book — a `ChangedEntitlement{IsRemoved:true}` —
    // rather than just no longer seeing it.
    let (app, pool, token, uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    opt_in(&pool, uid, std::slice::from_ref(&uuid)).await;

    let first = app
        .clone()
        .oneshot(get(format!("/kobo/{token}/v1/library/sync")))
        .await
        .unwrap();
    assert_eq!(body_json(first).await.as_array().unwrap().len(), 1);

    let shelves = db::shelves::list_visible_shelves(&pool, uid, false)
        .await
        .unwrap();
    let shelf_id = shelves
        .iter()
        .find(|s| s.name == "Kobo")
        .expect("seeded shelf")
        .id;
    db::shelves::update_shelf(
        &pool,
        shelf_id,
        &omnibus_shared::UpdateShelfRequest {
            sync_to_kobo: Some(false),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let second = app
        .clone()
        .oneshot(get(format!("/kobo/{token}/v1/library/sync")))
        .await
        .unwrap();
    let items = body_json(second).await;
    let arr = items.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    let removed = &arr[0]["ChangedEntitlement"]["BookEntitlement"];
    assert_eq!(removed["Id"], uuid);
    assert_eq!(removed["IsRemoved"], true);

    // And once the removal is delivered, the third sync is a true no-op.
    let third = app
        .oneshot(get(format!("/kobo/{token}/v1/library/sync")))
        .await
        .unwrap();
    assert!(body_json(third).await.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn library_sync_returns_an_empty_delta_once_the_device_is_current() {
    // The snapshot advances when the body drains, so an unchanged library
    // yields an empty second sync instead of a full re-download.
    let (app, pool, token, uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    opt_in(&pool, uid, std::slice::from_ref(&uuid)).await;

    let first = app
        .clone()
        .oneshot(get(format!("/kobo/{token}/v1/library/sync")))
        .await
        .unwrap();
    assert_eq!(body_json(first).await.as_array().unwrap().len(), 1);

    let second = app
        .oneshot(get(format!("/kobo/{token}/v1/library/sync")))
        .await
        .unwrap();
    assert!(body_json(second).await.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn library_sync_emits_the_change_pair_when_a_book_is_modified() {
    // A modified book re-syncs as ChangedProductMetadata + ChangedReadingState
    // — never a duplicate NewEntitlement, which would double the shelf row on
    // the device.
    let (app, pool, token, uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    opt_in(&pool, uid, std::slice::from_ref(&uuid)).await;
    let first = app
        .clone()
        .oneshot(get(format!("/kobo/{token}/v1/library/sync")))
        .await
        .unwrap();
    assert_eq!(body_json(first).await.as_array().unwrap().len(), 1);

    sqlx::query("UPDATE books SET last_modified = 9999999999 WHERE uuid = ?")
        .bind(&uuid)
        .execute(&pool)
        .await
        .unwrap();

    let second = app
        .oneshot(get(format!("/kobo/{token}/v1/library/sync")))
        .await
        .unwrap();
    let items = body_json(second).await;
    let arr = items.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert_eq!(
        arr[0]["ChangedProductMetadata"]["BookMetadata"]["EntitlementId"],
        uuid
    );
    assert_eq!(
        arr[1]["ChangedReadingState"]["ReadingState"]["EntitlementId"],
        uuid
    );
}

#[tokio::test]
async fn library_sync_advertises_the_source_epub_size() {
    let (app, pool, token, uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    sqlx::query(
        "UPDATE book_files SET size_bytes = 123456
          WHERE book_id = (SELECT id FROM books WHERE uuid = ?)",
    )
    .bind(&uuid)
    .execute(&pool)
    .await
    .unwrap();
    opt_in(&pool, uid, std::slice::from_ref(&uuid)).await;

    let res = app
        .oneshot(get(format!("/kobo/{token}/v1/library/sync")))
        .await
        .unwrap();
    let json = body_json(res).await;
    let ent = &json.as_array().unwrap()[0]["NewEntitlement"];

    assert_eq!(ent["BookMetadata"]["DownloadUrls"][0]["Size"], 123456);
}

#[tokio::test]
async fn library_sync_stamps_the_reading_state_clocks_on_a_new_entitlement() {
    // Same payload, second delivery path — the device adopts position from
    // either, so both must carry the arbitration clocks.
    let (app, pool, token, uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    opt_in(&pool, uid, std::slice::from_ref(&uuid)).await;
    pin_state_clocks(&pool, uid, &uuid, 1_700_000_000, 1_700_001_000).await;

    let res = app
        .oneshot(get(format!("/kobo/{token}/v1/library/sync")))
        .await
        .unwrap();
    let json = body_json(res).await;
    let rs = &json.as_array().unwrap()[0]["NewEntitlement"]["ReadingState"];

    assert_eq!(
        rs["CurrentBookmark"]["LastModified"],
        "2023-11-14T22:30:00Z"
    );
    assert_eq!(rs["StatusInfo"]["LastModified"], "2023-11-14T22:13:20Z");
    assert_eq!(rs["PriorityTimestamp"], "2023-11-14T22:30:00Z");
}

/// Pull the first `ReadingState` object out of a `library/sync` body.
fn first_reading_state(json: &Value) -> Option<Value> {
    json.as_array()?.iter().find_map(|item| {
        item.get("NewEntitlement")
            .or_else(|| item.get("ChangedReadingState"))
            .and_then(|e| e.get("ReadingState"))
            .cloned()
    })
}

#[tokio::test]
async fn library_sync_derives_a_kobospan_from_a_web_written_cfi() {
    let (app, pool, token, uid) = fixture().await;
    let (uuid, _guard, _lib) = seed_book_with_kepub_cache(&pool, "syncout", true).await;
    opt_in(&pool, uid, std::slice::from_ref(&uuid)).await;
    // A web-reader write: CFI only, percent/span cleared by replace-all.
    db::progress::upsert_progress(
        &pool,
        uid,
        &omnibus_shared::ProgressUpdate {
            book_uuid: uuid.clone(),
            format: omnibus_shared::ProgressFormat::Epub,
            epub_cfi: Some("epubcfi(/6/2!/4/4/1:0)".into()),
            audio_position_seconds: None,
            progress_percent: None,
            kobo_location: None,
            book_file_id: None,
            client_updated_at: Some(100),
        },
    )
    .await
    .unwrap();

    let res = app
        .clone()
        .oneshot(get(format!("/kobo/{token}/v1/library/sync")))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let json = body_json(res).await;
    let reading_state = first_reading_state(&json).expect("a reading state in the sync body");

    let bookmark = &reading_state["CurrentBookmark"];
    assert_eq!(
        bookmark["Location"]["Value"], "kobo.2.1",
        "the CFI at the second paragraph maps to its span: {bookmark}"
    );
    assert_eq!(bookmark["Location"]["Source"], "c1.xhtml");
    assert_eq!(bookmark["Location"]["Type"], "KoboSpan");
    // Anchor at char 24 of 53 visible chars → 45%.
    assert_eq!(bookmark["ProgressPercent"], 44);
    assert_eq!(
        reading_state["LastModified"], "1970-01-01T00:01:40Z",
        "the reading state must carry the position's own event time (100)"
    );

    // Write-back: the derived span is persisted clock-neutrally…
    let rec = db::progress::get_progress(&pool, uid, &uuid, omnibus_shared::ProgressFormat::Epub)
        .await
        .unwrap()
        .unwrap();
    assert!(rec.kobo_location.unwrap().contains("kobo.2.1"));
    assert_eq!(rec.progress_percent, Some(44));
    assert_eq!(
        rec.client_updated_at, 100,
        "write-back must not bump the clock"
    );

    // …so a second sync has nothing new to announce.
    let res = app
        .oneshot(get(format!("/kobo/{token}/v1/library/sync")))
        .await
        .unwrap();
    let json = body_json(res).await;
    assert!(
        json.as_array().unwrap().is_empty(),
        "derivation write-back must not re-fire the sync delta: {json}"
    );
}

#[tokio::test]
async fn library_sync_emits_percent_only_when_the_kepub_is_absent() {
    let (app, pool, token, uid) = fixture().await;
    let (uuid, _guard, _lib) = seed_book_with_kepub_cache(&pool, "nokepub", false).await;
    opt_in(&pool, uid, std::slice::from_ref(&uuid)).await;
    db::progress::upsert_progress(
        &pool,
        uid,
        &omnibus_shared::ProgressUpdate {
            book_uuid: uuid.clone(),
            format: omnibus_shared::ProgressFormat::Epub,
            epub_cfi: Some("epubcfi(/6/2!/4/4/1:0)".into()),
            audio_position_seconds: None,
            progress_percent: None,
            kobo_location: None,
            book_file_id: None,
            client_updated_at: Some(100),
        },
    )
    .await
    .unwrap();

    let res = app
        .oneshot(get(format!("/kobo/{token}/v1/library/sync")))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let json = body_json(res).await;
    let reading_state = first_reading_state(&json).expect("a reading state in the sync body");
    let bookmark = &reading_state["CurrentBookmark"];
    assert_eq!(
        bookmark["ProgressPercent"], 44,
        "the percent half needs only the source EPUB: {bookmark}"
    );
    assert!(
        bookmark.get("Location").is_none(),
        "no kepub → no span to invent: {bookmark}"
    );
    // Not persisted: the row still means "no span known" so a later sync
    // (with the queued conversion done) retries the full derivation.
    let rec = db::progress::get_progress(&pool, uid, &uuid, omnibus_shared::ProgressFormat::Epub)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rec.kobo_location, None);
}

#[tokio::test]
async fn library_sync_reports_real_read_status_and_position() {
    // AC1/AC2: a book finished and positioned on another surface syncs to a
    // fresh device carrying that status and percent, not the hardcoded default.
    let (app, pool, token, uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    opt_in(&pool, uid, std::slice::from_ref(&uuid)).await;
    db::read_status::set_read_status(
        &pool,
        uid,
        &omnibus_shared::SetReadStatus {
            book_uuid: uuid.clone(),
            status: omnibus_shared::ReadStatus::Finished,
        },
    )
    .await
    .unwrap();
    db::progress::upsert_progress(
        &pool,
        uid,
        &omnibus_shared::ProgressUpdate {
            book_uuid: uuid.clone(),
            format: omnibus_shared::ProgressFormat::Epub,
            epub_cfi: None,
            audio_position_seconds: None,
            progress_percent: Some(88),
            kobo_location: Some(r#"{"Value":"kobo.12.4"}"#.into()),
            client_updated_at: None,
            book_file_id: None,
        },
    )
    .await
    .unwrap();

    let res = app
        .oneshot(get(format!("/kobo/{token}/v1/library/sync")))
        .await
        .unwrap();

    let json = body_json(res).await;
    let rs = &json.as_array().unwrap()[0]["NewEntitlement"]["ReadingState"];
    assert_eq!(rs["StatusInfo"]["Status"], "Finished");
    assert_eq!(rs["CurrentBookmark"]["ProgressPercent"], 88);
    assert_eq!(rs["CurrentBookmark"]["Location"]["Value"], "kobo.12.4");
}

#[tokio::test]
async fn library_sync_reports_ready_to_read_for_an_untouched_book() {
    let (app, pool, token, uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    opt_in(&pool, uid, std::slice::from_ref(&uuid)).await;

    let res = app
        .oneshot(get(format!("/kobo/{token}/v1/library/sync")))
        .await
        .unwrap();

    let json = body_json(res).await;
    let rs = &json.as_array().unwrap()[0]["NewEntitlement"]["ReadingState"];
    assert_eq!(rs["StatusInfo"]["Status"], "ReadyToRead");
    // `CurrentBookmark` serializes its fields only when set, so an untouched
    // book emits an empty object rather than invented zeroes.
    assert!(rs["CurrentBookmark"]["ProgressPercent"].is_null());
}

#[tokio::test]
async fn library_sync_re_announces_a_status_change_to_a_device_that_holds_the_book() {
    // The state-only delta: metadata never moved, so without it a book
    // finished on the web would never reach a device that already has it.
    let (app, pool, token, uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    opt_in(&pool, uid, std::slice::from_ref(&uuid)).await;
    let first = app
        .clone()
        .oneshot(get(format!("/kobo/{token}/v1/library/sync")))
        .await
        .unwrap();
    assert_eq!(body_json(first).await.as_array().unwrap().len(), 1);

    // Age the snapshot past `synced_at`'s 1-second granularity.
    sqlx::query("UPDATE kobo_books_sync SET synced_at = 1")
        .execute(&pool)
        .await
        .unwrap();
    db::read_status::set_read_status(
        &pool,
        uid,
        &omnibus_shared::SetReadStatus {
            book_uuid: uuid.clone(),
            status: omnibus_shared::ReadStatus::Finished,
        },
    )
    .await
    .unwrap();

    let second = app
        .clone()
        .oneshot(get(format!("/kobo/{token}/v1/library/sync")))
        .await
        .unwrap();
    let json = body_json(second).await;
    let arr = json.as_array().unwrap();
    assert_eq!(arr.len(), 1, "a state change emits one bare item");
    assert_eq!(
        arr[0]["ChangedReadingState"]["ReadingState"]["StatusInfo"]["Status"],
        "Finished"
    );

    // And once delivered, the device is current again.
    let third = app
        .oneshot(get(format!("/kobo/{token}/v1/library/sync")))
        .await
        .unwrap();
    assert!(body_json(third).await.as_array().unwrap().is_empty());
}

// --- 500-on-DB-failure coverage (#1392) ---
//
// `KoboAuthUser` authenticates via `kobo_devices`, so dropping `books`
// leaves auth intact and forces the failure inside each handler's own
// `internal(...)` call site rather than at the extractor.

#[tokio::test]
async fn library_sync_returns_500_on_db_failure() {
    // `sync_books` short-circuits to an empty `Ok` when the user has no
    // opted-in shelf, never touching `books` — so a real opted-in book is
    // required to actually reach (and fail) that query.
    let (app, pool, token, uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    opt_in(&pool, uid, std::slice::from_ref(&uuid)).await;
    sqlx::query("DROP TABLE books")
        .execute(&pool)
        .await
        .unwrap();

    let res = app
        .oneshot(get(format!("/kobo/{token}/v1/library/sync")))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

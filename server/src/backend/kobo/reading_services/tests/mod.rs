//! HTTP-layer contract tests for the Reading Services annotation channel,
//! driving `reading_services_router` via `oneshot` against an in-memory DB.
//! The request builders and upload/checkforchanges helpers the sibling
//! modules share live here, alongside the DTO decoding tests.

mod device_flow;
mod downsync;

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    response::Response,
    Router,
};
use omnibus_db::{self as db};
use serde_json::{json, Value};
use sqlx::SqlitePool;
use tower::ServiceExt;

use super::*;
use crate::auth::test_support as auth_test_support;

const HW_ID: &str = "kobo-hw-0001";

/// Reading Services router on a fresh in-memory DB, plus the owning user's id
/// and a registered device whose hardware id is already learned — the state a
/// real device is in after its first tokened `/kobo/{token}/v1` call.
async fn fixture() -> (Router, SqlitePool, i64, i64) {
    let pool = db::init_db("sqlite::memory:").await.unwrap();
    let app = reading_services_router(AppState::new(pool.clone()));
    let user = auth_test_support::create_user(&pool, "kobo-reader").await;
    let device = db::kobo_devices::create_device(&pool, user.id, "Test Kobo")
        .await
        .unwrap();
    db::kobo_devices::learn_kobo_device_id(&pool, device.id, HW_ID)
        .await
        .unwrap();
    (app, pool, user.id, device.id)
}

async fn body_json(res: Response) -> Value {
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn request(method: &str, uri: &str, hw: Option<&str>, body: Option<&Value>) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(hw) = hw {
        builder = builder.header("x-kobo-deviceid", hw);
    }
    match body {
        Some(v) => builder
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(v).unwrap()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    }
}

fn annotations_uri(content_id: &str) -> String {
    format!("/api/v3/content/{content_id}/annotations")
}

fn upload_body(id: &str, color: &str, note: Option<&str>) -> Value {
    let mut annotation = json!({
        "id": id,
        "type": if note.is_some() { "note" } else { "highlight" },
        "highlightColor": color,
        "highlightedText": "the highlighted passage",
        "clientLastModifiedUtc": "2026-07-26T10:00:00Z",
        "location": {
            "span": {
                "chapterFilename": "OEBPS/ch1.xhtml",
                "startPath": "span#kobo\\.1\\.2",
                "startChar": 3,
                "endPath": "span#kobo\\.1\\.4",
                "endChar": 9
            }
        }
    });
    if let Some(note) = note {
        annotation["noteText"] = json!(note);
    }
    json!({ "updatedAnnotations": [annotation] })
}

/// PATCH one annotation and drain the GET so the pair is adopted and acked.
/// Marks `device` as having downloaded `uuid` first (#1647's ack gate) — a
/// device PATCHing highlights for a book realistically already holds it.
async fn upload_and_ack(
    app: &Router,
    pool: &SqlitePool,
    device: i64,
    uuid: &str,
    id: &str,
    color: &str,
    note: Option<&str>,
) {
    let res = app
        .clone()
        .oneshot(request(
            "PATCH",
            &annotations_uri(uuid),
            Some(HW_ID),
            Some(&upload_body(id, color, note)),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    db::kobo::annotations::mark_downloaded(pool, device, uuid)
        .await
        .unwrap();
    let res = app
        .clone()
        .oneshot(request("GET", &annotations_uri(uuid), Some(HW_ID), None))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let _ = body_json(res).await; // draining the body is what acks
}

mod dto_tests {
    use super::super::dto::*;
    use omnibus_shared::HighlightColor;
    use serde_json::json;

    #[test]
    fn parse_patch_accepts_variant_delete_shapes_and_deleted_flags() {
        let body = json!({
            "updatedAnnotations": [
                { "id": "keep", "highlightColor": "blue", "location": {} },
                { "id": "flagged", "isDeleted": true, "location": {} }
            ],
            "deletedAnnotationIds": ["bare-id"],
            "deletedAnnotations": [{ "id": "object-id" }],
            "removedAnnotations": ["removed-id"]
        });
        let parsed = parse_patch(&body);
        assert_eq!(parsed.skipped, 0);
        assert_eq!(parsed.updates.len(), 1);
        assert_eq!(parsed.updates[0].client_id, "keep");
        let mut deleted = parsed.deleted_ids.clone();
        deleted.sort();
        assert_eq!(
            deleted,
            vec!["bare-id", "flagged", "object-id", "removed-id"]
        );
    }

    #[test]
    fn parse_patch_skips_capped_and_idless_entries_without_dropping_the_rest() {
        let body = json!({
            "updatedAnnotations": [
                { "id": "ok", "location": {} },
                { "location": {} },
                { "id": "x".repeat(65), "location": {} },
                { "id": "long-note", "noteText": "n".repeat(4097), "location": {} },
                "not-an-object"
            ]
        });
        let parsed = parse_patch(&body);
        assert_eq!(parsed.updates.len(), 1);
        assert_eq!(parsed.updates[0].client_id, "ok");
        assert_eq!(parsed.skipped, 4);
    }

    #[test]
    fn parse_patch_treats_a_non_object_body_as_one_skip() {
        let parsed = parse_patch(&json!([1, 2, 3]));
        assert!(parsed.updates.is_empty());
        assert!(parsed.deleted_ids.is_empty());
        assert_eq!(parsed.skipped, 1);
    }

    #[test]
    fn parse_patch_lets_a_later_duplicate_win_and_a_delete_beat_an_update() {
        let body = json!({
            "updatedAnnotations": [
                { "id": "dup", "highlightColor": "blue", "location": {} },
                { "id": "dup", "highlightColor": "purple", "location": {} },
                { "id": "doomed", "highlightColor": "green", "location": {} }
            ],
            "deletedAnnotationIds": ["doomed"]
        });
        let parsed = parse_patch(&body);
        assert_eq!(parsed.updates.len(), 1);
        assert_eq!(parsed.updates[0].client_id, "dup");
        assert_eq!(parsed.updates[0].color, HighlightColor::Violet);
        assert_eq!(parsed.deleted_ids, vec!["doomed"]);
    }

    #[test]
    fn color_from_kobo_defaults_unrecognized_hex_and_missing_to_amber() {
        assert_eq!(color_from_kobo(Some("#FFDD00")), HighlightColor::Amber);
        assert_eq!(color_from_kobo(Some("chartreuse")), HighlightColor::Amber);
        assert_eq!(color_from_kobo(None), HighlightColor::Amber);
        assert_eq!(color_from_kobo(Some("PINK")), HighlightColor::Rose);
    }

    // #1629: wire value is a firmware hex swatch (see color_from_kobo_hex's doc comment); case-insensitive since the real device casing is unconfirmed.
    #[test]
    fn color_from_kobo_recognizes_every_kobo_firmware_hex_swatch_case_insensitively() {
        assert_eq!(color_from_kobo(Some("#F6F3B3")), HighlightColor::Amber);
        assert_eq!(color_from_kobo(Some("#c6e09e")), HighlightColor::Green);
        assert_eq!(color_from_kobo(Some("#B2E1E8")), HighlightColor::Blue);
        assert_eq!(color_from_kobo(Some("#e8afcf")), HighlightColor::Rose);
    }

    #[test]
    fn color_round_trip_is_stable_for_every_color_with_a_firmware_swatch() {
        // Amber, green, and blue have a swatch of their own. Rose's swatch
        // (pink) is also violet's nearest-neighbour target, so it round
        // trips but does not distinguish the two — see the next test.
        for color in [
            HighlightColor::Amber,
            HighlightColor::Green,
            HighlightColor::Blue,
            HighlightColor::Rose,
        ] {
            assert_eq!(color_from_kobo(Some(color_to_kobo(color))), color);
        }
    }

    #[test]
    fn color_round_trip_collapses_violet_into_rose_for_lack_of_a_fifth_kobo_swatch() {
        assert_eq!(
            color_to_kobo(HighlightColor::Violet),
            color_to_kobo(HighlightColor::Rose)
        );
        assert_eq!(
            color_from_kobo(Some(color_to_kobo(HighlightColor::Violet))),
            HighlightColor::Rose
        );
    }

    // Pins color_to_kobo's value mapping so a future change to it is deliberate.
    #[test]
    fn color_to_kobo_emits_the_documented_firmware_hex_swatch_for_every_palette_color() {
        assert_eq!(color_to_kobo(HighlightColor::Amber), "#F6F3B3");
        assert_eq!(color_to_kobo(HighlightColor::Green), "#C6E09E");
        assert_eq!(color_to_kobo(HighlightColor::Blue), "#B2E1E8");
        assert_eq!(color_to_kobo(HighlightColor::Rose), "#E8AFCF");
        assert_eq!(color_to_kobo(HighlightColor::Violet), "#E8AFCF");
    }

    #[test]
    fn parse_content_id_strips_the_chapter_suffix() {
        assert_eq!(parse_content_id("uuid-1!!OEBPS/ch1.xhtml"), "uuid-1");
        assert_eq!(parse_content_id("uuid-1"), "uuid-1");
        assert_eq!(parse_content_id(""), "");
    }
}

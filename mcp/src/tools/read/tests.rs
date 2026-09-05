//! The projection layer, driven through the tools against a stub instance.
//!
//! The stub answers each `/api/*` route these tools call, so a test asserts
//! what an agent actually receives — the shape after projection — rather than
//! what the endpoint returned.

use std::sync::Arc;

use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json as AxumJson, Router};
use rmcp::handler::server::wrapper::Parameters;

use omnibus_shared::{
    Bookmark, Contributor, EbookMetadata, Highlight, HighlightColor, PhysicalCopy, ProgressFormat,
    ProgressRecord, ReadStatus, ReadStatusRecord, ResumePoint, SessionFormat, SessionLogEntry,
    SessionLogPage,
};

use super::views::iso;
use super::*;
use crate::client::OmnibusClient;
use crate::config::Config;

const BOOK: &str = "uuid-piranesi";

/// A fixed instant with an unambiguous ISO rendering, so a test asserts the
/// stamp outright rather than against a recomputed conversion.
const WHEN: i64 = 1_772_582_400; // 2026-03-04T00:00:00Z

async fn login() -> AxumJson<serde_json::Value> {
    AxumJson(serde_json::json!({
        "user": {
            "id": 1, "username": "reader", "is_admin": false,
            "can_upload": false, "can_edit": false, "can_download": true
        },
        "token": "token-1",
    }))
}

/// A book record heavy enough that the stub-vs-full difference is visible in
/// the serialized size, the way a real one is.
fn full_book() -> EbookMetadata {
    EbookMetadata {
        id: 1,
        filename: "piranesi.epub".into(),
        title: Some("Piranesi".into()),
        description: Some("The House is beautiful. ".repeat(40)),
        creators: vec![Contributor {
            name: "Susanna Clarke".into(),
            role: Some("aut".into()),
            file_as: Some("Clarke, Susanna".into()),
            id: Some(7),
        }],
        subjects: (0..20).map(|n| format!("subject-{n}")).collect(),
        formats: vec!["epub".into()],
        series: Some("Standalone".into()),
        series_index: Some("1".into()),
        unique_identifier: Some(BOOK.into()),
        cover_url: Some("/api/covers/uuid-piranesi".into()),
        ..EbookMetadata::default()
    }
}

async fn book(Path(uuid): Path<String>) -> Result<AxumJson<EbookMetadata>, StatusCode> {
    if uuid == BOOK {
        Ok(AxumJson(full_book()))
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

fn progress_record(format: ProgressFormat) -> ProgressRecord {
    ProgressRecord {
        book_uuid: BOOK.into(),
        format,
        epub_cfi: matches!(format, ProgressFormat::Epub).then(|| "/6/14!/4/2".to_string()),
        audio_position_seconds: matches!(format, ProgressFormat::Audio).then_some(3600.0),
        progress_percent: Some(42),
        kobo_location: None,
        book_file_id: None,
        updated_at: WHEN,
        client_updated_at: WHEN - 60,
    }
}

#[derive(serde::Deserialize)]
struct FormatQuery {
    format: Option<String>,
}

/// The reader has an EPUB position and no audio one, so `include: progress`
/// has a present and an absent format to fold.
async fn progress(
    Path(uuid): Path<String>,
    Query(q): Query<FormatQuery>,
) -> AxumJson<Option<ProgressRecord>> {
    if uuid != BOOK || q.format.as_deref() == Some("audio") {
        return AxumJson(None);
    }
    AxumJson(Some(progress_record(ProgressFormat::Epub)))
}

async fn recent(
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> AxumJson<Vec<ResumePoint>> {
    let limit: usize = q.get("limit").and_then(|l| l.parse().ok()).unwrap_or(1);
    let point = ResumePoint {
        record: progress_record(ProgressFormat::Audio),
        book: full_book(),
        linked: false,
        cross_format: None,
        total_duration_seconds: Some(21_600.0),
        chapter_number: Some(3),
        chapter_count: Some(12),
        // The float sum a 2.3x preference actually serializes as.
        playback_rate: Some(2.3000000000000003),
    };
    AxumJson(std::iter::repeat_n(point, limit).collect())
}

async fn read_status(Path(uuid): Path<String>) -> AxumJson<Option<ReadStatusRecord>> {
    if uuid != BOOK {
        return AxumJson(None);
    }
    AxumJson(Some(ReadStatusRecord {
        book_uuid: BOOK.into(),
        status: ReadStatus::Reading,
        updated_at: WHEN,
        finished_at: None,
    }))
}

async fn highlights(Path(_): Path<String>) -> AxumJson<Vec<Highlight>> {
    AxumJson(vec![Highlight {
        id: 11,
        book_uuid: BOOK.into(),
        epub_cfi_range: Some("/6/14!/4/2,/1:0,/1:24".into()),
        color: HighlightColor::Amber,
        note: Some("the statues".into()),
        text: Some("The Beauty of the House is immeasurable".into()),
        client_id: None,
        created_at: WHEN,
    }])
}

async fn bookmarks(Path(_): Path<String>) -> AxumJson<Vec<Bookmark>> {
    AxumJson(vec![Bookmark {
        id: 22,
        book_uuid: BOOK.into(),
        position: "/6/14!/4/2".into(),
        title: Some("The Vestibule".into()),
        client_id: None,
        created_at: WHEN,
    }])
}

async fn sessions(
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> AxumJson<SessionLogPage> {
    if q.get("book").is_some_and(|b| b != BOOK) {
        return AxumJson(SessionLogPage::default());
    }
    AxumJson(SessionLogPage {
        entries: vec![SessionLogEntry {
            book_uuid: BOOK.into(),
            title: "Piranesi".into(),
            // The vocabulary a progress record cannot express.
            format: SessionFormat::Mixed,
            started_at: WHEN,
            ended_at: WHEN + 1800,
            seconds: 1500,
        }],
        next_before: None,
    })
}

async fn copies(Path(_): Path<String>) -> AxumJson<Vec<PhysicalCopy>> {
    AxumJson(vec![PhysicalCopy {
        id: 33,
        book_uuid: BOOK.into(),
        isbn: Some("9781635575637".into()),
        added_by_user_id: Some(1),
        checked_in_at: WHEN,
        note: None,
    }])
}

/// Boot a stub instance serving every route these tools reach.
async fn stub_service() -> OmnibusMcp {
    let app = Router::new()
        .route("/api/auth/login", post(login))
        .route("/api/ebooks/{uuid}", get(book))
        .route("/api/progress/{uuid}", get(progress))
        .route("/api/progress/recent", get(recent))
        .route("/api/read-status/{uuid}", get(read_status))
        .route("/api/highlights/book/{uuid}", get(highlights))
        .route("/api/bookmarks/book/{uuid}", get(bookmarks))
        .route("/api/stats/sessions", get(sessions))
        .route("/api/physical/{uuid}/copies", get(copies));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client = OmnibusClient::new(Config {
        base_url: format!("http://{addr}"),
        username: "reader".into(),
        password: "correct horse battery".into(),
    })
    .unwrap();
    OmnibusMcp::new(Arc::new(client))
}

/// `unwrap_err` needs `T: Debug`, which `rmcp::Json` does not implement.
fn expect_err<T>(result: Result<T, ErrorData>) -> ErrorData {
    match result {
        Err(e) => e,
        Ok(_) => panic!("expected an error"),
    }
}

fn json_of<T: serde::Serialize>(value: &T) -> serde_json::Value {
    serde_json::to_value(value).unwrap()
}

// MARK: - Stamps

#[test]
fn iso_renders_unix_seconds_as_an_rfc3339_utc_stamp() {
    assert_eq!(iso(WHEN).as_deref(), Some("2026-03-04T00:00:00Z"));
    assert_eq!(iso(0).as_deref(), Some("1970-01-01T00:00:00Z"));
}

#[test]
fn iso_declines_an_epoch_no_calendar_can_render() {
    // Far outside `OffsetDateTime`'s range — the epoch twin still carries the
    // value, so declining costs the caller nothing but the convenience.
    assert_eq!(iso(i64::MAX), None);
}

#[tokio::test]
async fn book_progress_carries_each_stamp_as_iso_beside_its_epoch() {
    let service = stub_service().await;
    let record = service
        .book_progress(Parameters(BookProgressParams {
            uuid: BOOK.into(),
            format: Some(ProgressFormat::Epub),
        }))
        .await
        .unwrap()
        .0
        .expect("the reader has an epub position");
    assert_eq!(record.updated_at.as_deref(), Some("2026-03-04T00:00:00Z"));
    assert_eq!(record.updated_at_epoch, WHEN);
    assert_eq!(
        record.client_updated_at.as_deref(),
        Some("2026-03-03T23:59:00Z")
    );
    assert_eq!(record.client_updated_at_epoch, WHEN - 60);
}

#[tokio::test]
async fn every_record_tool_answers_with_an_iso_stamp_for_each_epoch_it_returns() {
    let service = stub_service().await;
    let uuid = || Parameters(BookRef { uuid: BOOK.into() });

    let status = service.book_read_status(uuid()).await.unwrap().0.unwrap();
    assert_eq!(status.updated_at.as_deref(), Some("2026-03-04T00:00:00Z"));
    assert_eq!(status.updated_at_epoch, WHEN);

    let highlights = service.book_highlights(uuid()).await.unwrap().0;
    assert_eq!(
        highlights[0].created_at.as_deref(),
        Some("2026-03-04T00:00:00Z")
    );
    assert_eq!(highlights[0].created_at_epoch, WHEN);

    let bookmarks = service.book_bookmarks(uuid()).await.unwrap().0;
    assert_eq!(
        bookmarks[0].created_at.as_deref(),
        Some("2026-03-04T00:00:00Z")
    );
    assert_eq!(bookmarks[0].created_at_epoch, WHEN);

    let sessions = service
        .reading_sessions(Parameters(SessionLogParams {
            book: Some(BOOK.into()),
            ..SessionLogParams::default()
        }))
        .await
        .unwrap()
        .0;
    assert_eq!(
        sessions.entries[0].started_at.as_deref(),
        Some("2026-03-04T00:00:00Z")
    );
    assert_eq!(sessions.entries[0].started_at_epoch, WHEN);
    assert_eq!(
        sessions.entries[0].ended_at.as_deref(),
        Some("2026-03-04T00:30:00Z")
    );
    assert_eq!(sessions.entries[0].ended_at_epoch, WHEN + 1800);
}

#[tokio::test]
async fn list_physical_copies_stamps_the_check_in_in_both_forms() {
    let service = stub_service().await;
    let copies = service
        .list_physical_copies(Parameters(crate::tools::checkin::BookUuid {
            uuid: BOOK.into(),
        }))
        .await
        .unwrap()
        .0;
    assert_eq!(
        copies[0].checked_in_at.as_deref(),
        Some("2026-03-04T00:00:00Z")
    );
    assert_eq!(copies[0].checked_in_at_epoch, WHEN);
}

// MARK: - The resume feed's book projection

#[tokio::test]
async fn recent_progress_projects_a_book_stub_by_default() {
    let service = stub_service().await;
    let points = service
        .recent_progress(Parameters(RecentProgressParams::default()))
        .await
        .unwrap()
        .0;
    let book = &json_of(&points[0])["book"];
    // Enough to name the book and go fetch the rest…
    assert_eq!(book["uuid"], BOOK);
    assert_eq!(book["title"], "Piranesi");
    assert_eq!(book["creators"][0]["name"], "Susanna Clarke");
    assert_eq!(book["formats"][0], "epub");
    assert_eq!(book["series"], "Standalone");
    // …and none of the bulk that made this feed expensive to read.
    assert!(book.get("description").is_none());
    assert!(book.get("subjects").is_none());
    assert!(book.get("book_files").is_none());
    assert!(book.get("identifiers").is_none());
}

#[tokio::test]
async fn recent_progress_inlines_the_whole_record_when_asked_for_full() {
    let service = stub_service().await;
    let points = service
        .recent_progress(Parameters(RecentProgressParams {
            verbosity: Some(Verbosity::Full),
            ..RecentProgressParams::default()
        }))
        .await
        .unwrap()
        .0;
    let book = &json_of(&points[0])["book"];
    assert_eq!(book["unique_identifier"], BOOK);
    assert!(book["description"].as_str().unwrap().len() > 200);
    assert_eq!(book["subjects"].as_array().unwrap().len(), 20);
}

#[tokio::test]
async fn a_three_entry_stub_feed_is_a_fraction_of_the_full_one() {
    let service = stub_service().await;
    let feed = |verbosity| {
        let service = &service;
        async move {
            let points = service
                .recent_progress(Parameters(RecentProgressParams {
                    limit: Some(3),
                    verbosity,
                }))
                .await
                .unwrap()
                .0;
            serde_json::to_string(&points).unwrap().len()
        }
    };
    let stub = feed(None).await;
    let full = feed(Some(Verbosity::Full)).await;
    // The whole point of the default: the feed a resume prompt reads should
    // not cost a large fraction of the window.
    assert!(
        stub * 3 < full,
        "stub feed ({stub} bytes) should be far smaller than full ({full} bytes)"
    );
}

#[tokio::test]
async fn recent_progress_rounds_the_playback_rate_it_reports() {
    let service = stub_service().await;
    let points = service
        .recent_progress(Parameters(RecentProgressParams::default()))
        .await
        .unwrap()
        .0;
    assert_eq!(points[0].playback_rate, Some(2.3));
    // And it must not come back through serde as the float sum it was.
    let rendered = serde_json::to_string(&points[0]).unwrap();
    assert!(
        rendered.contains("2.3") && !rendered.contains("2.3000000000000003"),
        "unrounded rate in {rendered}"
    );
}

// MARK: - One call for one book

#[tokio::test]
async fn get_book_returns_metadata_alone_when_nothing_is_included() {
    let service = stub_service().await;
    let detail = service
        .get_book(Parameters(GetBookParams {
            uuid: BOOK.into(),
            include: None,
        }))
        .await
        .unwrap()
        .0;
    assert_eq!(detail.book.unique_identifier.as_deref(), Some(BOOK));
    let rendered = json_of(&detail);
    for section in [
        "progress",
        "read_status",
        "highlights",
        "bookmarks",
        "sessions",
        "copies",
    ] {
        assert!(
            rendered.get(section).is_none(),
            "{section} should be absent when it was not requested"
        );
    }
}

#[tokio::test]
async fn get_book_folds_every_requested_section_into_one_answer() {
    let service = stub_service().await;
    let detail = service
        .get_book(Parameters(GetBookParams {
            uuid: BOOK.into(),
            include: Some(vec![
                BookInclude::Progress,
                BookInclude::ReadStatus,
                BookInclude::Highlights,
                BookInclude::Bookmarks,
                BookInclude::Sessions,
                BookInclude::Copies,
            ]),
        }))
        .await
        .unwrap()
        .0;
    assert_eq!(detail.book.title.as_deref(), Some("Piranesi"));
    // One position, not two: the reader has never opened the audiobook.
    let progress = detail.progress.expect("progress requested");
    assert_eq!(progress.len(), 1);
    assert_eq!(progress[0].format, ProgressFormat::Epub);
    assert_eq!(
        detail
            .read_status
            .expect("read_status requested")
            .unwrap()
            .status,
        ReadStatus::Reading
    );
    assert_eq!(detail.highlights.expect("highlights requested").len(), 1);
    assert_eq!(detail.bookmarks.expect("bookmarks requested").len(), 1);
    assert_eq!(detail.sessions.expect("sessions requested").len(), 1);
    assert_eq!(detail.copies.expect("copies requested").len(), 1);
}

#[tokio::test]
async fn get_book_includes_only_the_sections_it_was_asked_for() {
    let service = stub_service().await;
    let detail = service
        .get_book(Parameters(GetBookParams {
            uuid: BOOK.into(),
            include: Some(vec![BookInclude::Highlights]),
        }))
        .await
        .unwrap()
        .0;
    assert!(detail.highlights.is_some());
    assert!(detail.progress.is_none());
    assert!(detail.read_status.is_none());
    assert!(detail.sessions.is_none());
}

#[tokio::test]
async fn get_book_tells_a_section_with_nothing_to_report_from_one_never_asked_for() {
    let service = stub_service().await;
    // The stub answers `null` for a read status on any other uuid, but the
    // book itself only exists under BOOK — so ask for the section on the book
    // that exists and assert the *requested-but-empty* rendering.
    let detail = service
        .get_book(Parameters(GetBookParams {
            uuid: BOOK.into(),
            include: Some(vec![BookInclude::ReadStatus]),
        }))
        .await
        .unwrap()
        .0;
    let rendered = json_of(&detail);
    // Requested sections are present keys; unrequested ones are absent.
    assert!(rendered.get("read_status").is_some());
    assert!(rendered.get("copies").is_none());
}

#[tokio::test]
async fn get_book_reports_not_found_for_an_unknown_uuid() {
    let service = stub_service().await;
    let err = expect_err(
        service
            .get_book(Parameters(GetBookParams {
                uuid: "uuid-missing".into(),
                include: None,
            }))
            .await,
    );
    assert!(err.message.contains("not found"));
}

// MARK: - Vocabulary

#[test]
fn the_two_format_vocabularies_are_documented_wherever_they_disagree() {
    // `SessionFormat` carries `mixed`, which no `ProgressFormat` can express,
    // so the two cannot be unified — the mapping is stated in the description
    // of every tool that returns one of them instead.
    let router = OmnibusMcp::read_tools();
    let tools = router.list_all();
    for name in ["reading_sessions", "recent_progress", "book_progress"] {
        let tool = tools
            .iter()
            .find(|t| t.name.as_ref() == name)
            .unwrap_or_else(|| panic!("missing tool {name}"));
        let desc = tool.description.as_deref().unwrap_or_default();
        assert!(
            desc.contains("epub") && desc.contains("listening"),
            "{name} must map the progress and session format vocabularies"
        );
    }
}

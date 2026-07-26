//! Wire DTOs for the Kobo wireless protocol, PascalCase-shaped to match what
//! the device sends and expects. Models the entitlement envelope emitted by
//! `library/sync` plus the request/response for the `state` PUT.

use omnibus_db::kobo::KoboBookRow;
use serde::{Deserialize, Serialize};

/// Format an epoch-seconds instant as the RFC 3339 string Kobo expects.
/// Falls back to the Unix epoch for an out-of-range value rather than erroring.
pub fn rfc3339(epoch: i64) -> String {
    time::OffsetDateTime::from_unix_timestamp(epoch)
        .unwrap_or(time::OffsetDateTime::UNIX_EPOCH)
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned())
}

/// One element of the `library/sync` array. Externally tagged, so it
/// serializes as `{"NewEntitlement": { … }}` — the shape the device parses.
#[derive(Debug, Serialize)]
pub enum SyncItem {
    NewEntitlement(Entitlement),
}

/// A full entitlement: the ownership record, book metadata, and reading state.
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct Entitlement {
    pub book_entitlement: BookEntitlement,
    pub book_metadata: BookMetadata,
    pub reading_state: ReadingState,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct BookEntitlement {
    pub id: String,
    pub cross_revision_id: String,
    pub revision_id: String,
    pub created: String,
    pub last_modified: String,
    pub status: &'static str,
    pub accessibility: &'static str,
    pub is_removed: bool,
    pub is_hidden_from_archive: bool,
    pub is_locked: bool,
    pub origin_category: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct BookMetadata {
    pub entitlement_id: String,
    pub cross_revision_id: String,
    pub revision_id: String,
    pub title: String,
    pub description: String,
    pub language: String,
    pub cover_image_id: String,
    pub slug: String,
    pub download_urls: Vec<DownloadUrl>,
    pub contributor_roles: Vec<Contributor>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct Contributor {
    pub name: String,
    pub role: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct DownloadUrl {
    pub format: &'static str,
    pub size: u64,
    pub url: String,
    pub platform: &'static str,
    pub drm_type: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ReadingState {
    pub entitlement_id: String,
    pub created: String,
    pub last_modified: String,
    pub status_info: StatusInfo,
    pub current_bookmark: CurrentBookmark,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct StatusInfo {
    /// `ReadyToRead` | `Reading` | `Finished`.
    pub status: String,
}

#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct CurrentBookmark {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress_percent: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_source_progress_percent: Option<i64>,
    /// Opaque `KoboSpan` position anchor (`kobo.N.M`). Slice A round-trips it
    /// verbatim but does **not** convert it to an EPUB CFI — see #925.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<serde_json::Value>,
}

/// Build a first-connect `NewEntitlement` for `book`. `base` is the absolute
/// origin (e.g. `https://host`) and `token` the device path token, so the
/// download URL points back at this server's Kobo route.
pub fn new_entitlement(base: &str, token: &str, book: &KoboBookRow, size: u64) -> SyncItem {
    let ts = rfc3339(book.last_modified_epoch);
    let uuid = book.uuid.clone();
    SyncItem::NewEntitlement(Entitlement {
        book_entitlement: BookEntitlement {
            id: uuid.clone(),
            cross_revision_id: uuid.clone(),
            revision_id: uuid.clone(),
            created: ts.clone(),
            last_modified: ts.clone(),
            status: "Active",
            accessibility: "Full",
            is_removed: false,
            is_hidden_from_archive: false,
            is_locked: false,
            origin_category: "Imported",
        },
        book_metadata: book_metadata(base, token, book, size),
        reading_state: ReadingState {
            entitlement_id: uuid.clone(),
            created: ts.clone(),
            last_modified: ts,
            status_info: StatusInfo {
                status: "ReadyToRead".to_owned(),
            },
            current_bookmark: CurrentBookmark::default(),
        },
    })
}

/// Build the `BookMetadata` for `book`, with a `DownloadUrl` pointing back at
/// this server's Kobo download route. Shared by `library/sync` and the
/// `library/<uuid>/metadata` endpoint so the two never drift.
pub fn book_metadata(base: &str, token: &str, book: &KoboBookRow, size: u64) -> BookMetadata {
    let uuid = book.uuid.clone();
    BookMetadata {
        entitlement_id: uuid.clone(),
        cross_revision_id: uuid.clone(),
        revision_id: uuid.clone(),
        title: book.title.clone(),
        description: String::new(),
        language: "en".to_owned(),
        cover_image_id: uuid.clone(),
        slug: uuid.clone(),
        download_urls: vec![DownloadUrl {
            format: "KEPUB",
            size,
            url: format!("{base}/kobo/{token}/v1/download/{uuid}"),
            platform: "Generic",
            drm_type: "None",
        }],
        contributor_roles: vec![Contributor {
            name: book.author.clone(),
            role: "Author",
        }],
    }
}

/// The `PUT library/<uuid>/state` request body. Kobo batches one or more
/// reading states; slice A reads `StatusInfo`/`CurrentBookmark` and ignores
/// `Statistics`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct StateRequest {
    #[serde(default)]
    pub reading_states: Vec<StateEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct StateEntry {
    #[serde(default)]
    pub status_info: Option<StatusInfo>,
    #[serde(default)]
    pub current_bookmark: Option<CurrentBookmark>,
}

/// The `state` PUT response. Kobo checks each sub-result is `Success`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct StateResponse {
    pub request_result: &'static str,
    pub update_results: Vec<UpdateResult>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct UpdateResult {
    pub entitlement_id: String,
    pub status_info_result: ResultTag,
    pub current_bookmark_result: ResultTag,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ResultTag {
    pub result: &'static str,
}

impl StateResponse {
    /// A blanket `Success` response for `entitlement_id`.
    pub fn success(entitlement_id: String) -> Self {
        Self {
            request_result: "Success",
            update_results: vec![UpdateResult {
                entitlement_id,
                status_info_result: ResultTag { result: "Success" },
                current_bookmark_result: ResultTag { result: "Success" },
            }],
        }
    }
}

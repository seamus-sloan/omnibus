//! Wire DTOs for the Kobo wireless protocol, PascalCase-shaped to match what
//! the device sends and expects. Models the entitlement envelope emitted by
//! `library/sync` plus the request/response for the `state` PUT.

use omnibus_db::kobo::{KoboBookRow, KoboBookState};
use omnibus_shared::ReadStatus;
use serde::{Deserialize, Serialize};

/// Format an epoch-seconds instant as the RFC 3339 string Kobo expects.
/// Falls back to the Unix epoch for an out-of-range value rather than erroring.
pub fn rfc3339(epoch: i64) -> String {
    time::OffsetDateTime::from_unix_timestamp(epoch)
        .unwrap_or(time::OffsetDateTime::UNIX_EPOCH)
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned())
}

/// The `v1/auth/device` / `v1/auth/refresh` response envelope.
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct AuthEnvelope {
    pub access_token: String,
    pub refresh_token: String,
    pub token_type: String,
    pub tracking_id: String,
    pub user_key: String,
}

/// Build the auth envelope for a device.
///
/// The values are **derived from the path token, not random**. Nothing ever
/// validates them — the `/kobo/<TOKEN>/` path token is the real credential —
/// and the device re-runs this handshake on a refresh schedule, so a stable
/// answer is the honest one: it needs no storage and cannot drift between the
/// initial exchange and a later refresh.
pub fn auth_envelope(token: &str) -> AuthEnvelope {
    AuthEnvelope {
        access_token: derive_opaque(token, "access"),
        refresh_token: derive_opaque(token, "refresh"),
        token_type: "Bearer".to_owned(),
        tracking_id: derive_opaque(token, "tracking"),
        user_key: derive_opaque(token, "user"),
    }
}

/// A stable opaque hex value for `(token, purpose)`. Not a secret and not
/// treated as one — it exists to fill a field shape the device requires.
///
/// SHA-256 rather than `DefaultHasher`: the latter's output is explicitly not
/// stable across Rust toolchain versions, so a compiler bump would silently
/// rotate every device's envelope and defeat the stability this function
/// exists to provide. `db::helpers::stable_uuid` documents the same trap after
/// a `DefaultHasher` rotation once orphaned every cover file on disk.
fn derive_opaque(token: &str, purpose: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    // NUL can't appear in either input, so it's an unambiguous separator — no
    // other `(purpose, token)` split can produce the same pre-image.
    hasher.update(purpose.as_bytes());
    hasher.update([0u8]);
    hasher.update(token.as_bytes());
    let digest = hasher.finalize();
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// One element of the `library/sync` array. Externally tagged, so each
/// serializes as `{"<Shape>": { … }}` — the envelope names the device parses.
///
/// The three shapes map 1:1 onto `db::kobo::SyncChange`: an add is a
/// `NewEntitlement`; a change is `ChangedProductMetadata` (the device
/// re-fetches metadata + file) paired with a `ChangedReadingState`; a removal
/// is a `ChangedEntitlement` whose `BookEntitlement.IsRemoved` is true, which
/// archives the book on-device without touching annotations.
#[derive(Debug, Serialize)]
pub enum SyncItem {
    NewEntitlement(Entitlement),
    ChangedEntitlement(Entitlement),
    ChangedProductMetadata(ChangedProductMetadata),
    ChangedReadingState(ChangedReadingState),
}

/// `ChangedProductMetadata` payload: just the refreshed metadata block.
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ChangedProductMetadata {
    pub book_metadata: BookMetadata,
}

/// `ChangedReadingState` payload: just the reading-state block.
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ChangedReadingState {
    pub reading_state: ReadingState,
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
pub fn new_entitlement(
    base: &str,
    token: &str,
    book: &KoboBookRow,
    state: Option<&KoboBookState>,
) -> SyncItem {
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
        book_metadata: book_metadata(base, token, book),
        reading_state: reading_state(&uuid, &ts, state),
    })
}

/// The device-facing reading state for one book: real status and position when
/// the user has any, an untouched book's defaults otherwise.
fn reading_state(uuid: &str, ts: &str, state: Option<&KoboBookState>) -> ReadingState {
    ReadingState {
        entitlement_id: uuid.to_owned(),
        created: ts.to_owned(),
        last_modified: ts.to_owned(),
        status_info: StatusInfo {
            status: kobo_status(state).to_owned(),
        },
        current_bookmark: state
            .map(|s| CurrentBookmark {
                progress_percent: s.percent,
                content_source_progress_percent: None,
                // Round-tripped verbatim; a value we did not store parses back
                // to nothing rather than being invented.
                location: s
                    .kobo_location
                    .as_deref()
                    .and_then(|raw| serde_json::from_str(raw).ok()),
            })
            .unwrap_or_default(),
    }
}

/// Map the internal read status onto the device's vocabulary. `Unread` is
/// `ReadyToRead` on the wire — Kobo has no "unread" token.
fn kobo_status(state: Option<&KoboBookState>) -> &'static str {
    match state.map(|s| s.status) {
        Some(ReadStatus::Finished) => "Finished",
        Some(ReadStatus::Reading) => "Reading",
        Some(ReadStatus::Unread) | None => "ReadyToRead",
    }
}

/// The `SyncItem`s for one `db::kobo::SyncChange`. A change fans out into two
/// items (metadata + reading state); an add or removal is one.
pub fn sync_items(
    base: &str,
    token: &str,
    change: &omnibus_db::kobo::SyncChange,
    state: Option<&KoboBookState>,
) -> Vec<SyncItem> {
    use omnibus_db::kobo::SyncChange;
    match change {
        SyncChange::New(book) => vec![new_entitlement(base, token, book, state)],
        SyncChange::Changed(book) => {
            let ts = rfc3339(book.last_modified_epoch);
            vec![
                SyncItem::ChangedProductMetadata(ChangedProductMetadata {
                    book_metadata: book_metadata(base, token, book),
                }),
                SyncItem::ChangedReadingState(ChangedReadingState {
                    reading_state: reading_state(&book.uuid, &ts, state),
                }),
            ]
        }
        // Metadata is already current on the device; only the state moved.
        SyncChange::StateChanged(book) => {
            let ts = rfc3339(book.last_modified_epoch);
            vec![SyncItem::ChangedReadingState(ChangedReadingState {
                reading_state: reading_state(&book.uuid, &ts, state),
            })]
        }
        SyncChange::Removed { book_uuid } => vec![removed_entitlement(book_uuid)],
    }
}

/// A `ChangedEntitlement` that archives `book_uuid` on the device
/// (`IsRemoved: true`). Metadata is a minimal shell — the device only needs
/// the ids to find what to drop.
fn removed_entitlement(book_uuid: &str) -> SyncItem {
    let uuid = book_uuid.to_owned();
    let ts = rfc3339(0);
    SyncItem::ChangedEntitlement(Entitlement {
        book_entitlement: BookEntitlement {
            id: uuid.clone(),
            cross_revision_id: uuid.clone(),
            revision_id: uuid.clone(),
            created: ts.clone(),
            last_modified: ts.clone(),
            status: "Deleted",
            accessibility: "Full",
            is_removed: true,
            is_hidden_from_archive: false,
            is_locked: false,
            origin_category: "Imported",
        },
        book_metadata: BookMetadata {
            entitlement_id: uuid.clone(),
            cross_revision_id: uuid.clone(),
            revision_id: uuid.clone(),
            title: String::new(),
            description: String::new(),
            language: "en".to_owned(),
            cover_image_id: uuid.clone(),
            slug: uuid.clone(),
            download_urls: Vec::new(),
            contributor_roles: Vec::new(),
        },
        reading_state: ReadingState {
            entitlement_id: uuid,
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
pub fn book_metadata(base: &str, token: &str, book: &KoboBookRow) -> BookMetadata {
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
            // Best-effort: the source EPUB's size (the served KEPUB differs
            // slightly). Only drives device-side progress UI.
            size: book.epub_size_bytes.max(0) as u64,
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

/// Cumulative per-book stats on the `state` PUT. Parsed so the payload
/// round-trips cleanly, but **deliberately not written anywhere**: these are
/// running totals, while `reading_sessions` stores discrete sessions — the
/// per-session truth already arrives via the `LeaveContent` analytics event,
/// and deriving sessions from a cumulative counter would double-count.
#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct Statistics {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spent_reading_minutes: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remaining_time_minutes: Option<i64>,
}

/// The `PUT library/<uuid>/state` request body. Kobo batches one or more
/// reading states; `StatusInfo` persists, `CurrentBookmark` waits on the
/// KoboSpan decision (#925), `Statistics` is acknowledged (see its doc).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct StateRequest {
    #[serde(default)]
    pub reading_states: Vec<StateEntry>,
}

impl StateRequest {
    /// Maximum number of `reading_states` entries accepted per `state` PUT.
    /// The router's `DefaultBodyLimit` already bounds total payload bytes;
    /// this bounds per-request handler work (a DB round trip per entry)
    /// independent of how small an individual entry is.
    pub const MAX_READING_STATES: usize = 128;
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct StateEntry {
    #[serde(default)]
    pub status_info: Option<StatusInfo>,
    #[serde(default)]
    pub current_bookmark: Option<CurrentBookmark>,
    #[serde(default)]
    pub statistics: Option<Statistics>,
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
